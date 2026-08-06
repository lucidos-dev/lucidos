//! LLM-facing schemas for the thread tools a parent uses to run its own
//! fan-out: the two spawn tools (run_thread, run_coding_agent) and the
//! follow-up that redirects a child already spawned
//! (follow_up_child_thread). All three are standalone per the
//! hot-single-purpose guardrail asserted in `llm/tools/tests.rs`.
//!
//! Thread INTROSPECTION (list_threads / count_threads) is the grouped `threads`
//! manifest tool (built from `crate::capability_manifest`); the flat names stay
//! wired as back-compat aliases in `execute_tool`.

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
                        "description": "Short descriptive title (3-6 words) for the thread. When provided, the system will not auto-generate a title. Set it: this is how you will refer to this child later, in your own prose and in the result of follow_up_child_thread, and it is what makes the thread list meaningful at a glance."
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
            description: "Run a coding agent to edit source code. Default backend is Claude Code; set `coding_agent=\"codex\"` whenever the user asks for Codex or OpenAI Codex. ONLY use this when the user explicitly asks to modify code (Lucidos, an installed app, or an external repo). Never use this for workspace tasks like web scraping, file manipulation, data processing, or anything the native tools can handle — use browser_open, web_search, http_request, run_python, read_file, write_file, etc. instead.\n\nDefault behavior (relation=\"child\") is a child thread: spawning returns immediately; when the coding-agent session ends, a callback automatically resumes THIS thread with the child thread's final response text — you do NOT need to poll. For PARALLEL work, issue multiple run_coding_agent calls in one response and they spawn concurrently, each reporting back independently. For SEQUENTIAL pipelines (where step N depends on step N-1's outcome — e.g., build → harden → e2e, stopping on first failure), spawn ONE run_coding_agent and end the turn; the next step runs only after the callback resumes you. Never batch sequential spawns in one response — that defeats the dependency. Always inspect the child thread's final response text to determine pass/fail before acting on it or emitting milestones — the spawn ack is not a result.\n\nPass relation=\"top\" instead when the user asks for a piece of work to happen in its own thread that they will follow themselves (e.g. 'do this in a separate thread' / 'spawn a Codex session for this and I'll check in later'). The spawned coding-agent session runs as an independent top-level thread and will NOT report back to this conversation.\n\nCROSS-WORKSPACE: When the user asks for the work to happen in a DIFFERENT workspace ('do this in dev', 'fix it in myws'), set the `workspace` parameter to the target workspace's basename (e.g. `workspace=\"dev\"`). The tool will POST to that workspace's engine and the coding-agent session will land there — same UX as a local spawn. Cross-workspace requires `relation=\"top\"` (child-thread auto-resume callbacks across workspaces are unsupported); the tool refuses child + cross-workspace with an error. The `folder` parameter resolves on the target workspace's filesystem; for cross-workspace spawns make sure the target workspace has the app installed or the repo registered.\n\nBEFORE CALLING: identify what the work targets and pick the right `folder`.\n- Editing Lucidos itself (engine/UI under the Lucidos source tree) → omit `folder`. AVAILABLE ONLY on an install whose engine was launched from a Lucidos source checkout. The system prompt's \"WHAT A CODING AGENT CAN EDIT ON THIS INSTALL\" section states which install this is; when it says there is no source checkout, a `folder`-less call against THIS install is refused — do not claim you can change Lucidos itself, and do not tell the user to Apply or restart for it. A `folder`-less call WITH `workspace=\"<other>\"` is unaffected: it is forwarded to that workspace's engine, which applies its own source check.\n- Editing an installed app's UI/knowhow/intents → pass `folder=\"data/apps/<id>\"` (workspace-relative; the engine resolves against the target workspace's root). The session runs as an *app coding-agent thread*: sparse-checkout worktree narrowed to that app folder, Apply ff-merges to the workspace git's main, no engine restart, no `/harden`. For tiny edits that don't need a full agent worktree, use the chat path (file tools + the `lucidos` CLI) instead.\n- Editing a registered external repo → pass `folder=<repo name>` (resolves via the repo registry, same as the deprecated `repo` param).\n- Editing an unregistered git folder or a non-git directory → not supported in v1; refused with a clear error. Register the repo with `manage_repositories action='add'` first.\n- Editing a `data/` subtree other than `data/apps/<id>/` (knowhow, triggers, artifacts, config, scripts) → refused. Use the chat path: file tools + the `lucidos` CLI.\n- Ambiguous → ask one question: 'which folder should this run in?' before calling.\n\nIf Rust backend files are changed, Lucidos shows the user a toast suggesting a restart — the user must manually trigger the rebuild and restart for backend changes to take effect (do NOT promise that changes will be live shortly). Frontend (TypeScript/CSS) changes are picked up automatically. App `data/apps/<id>/` changes never restart the engine.".to_string(),
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
                        "description": "What the coding-agent thread should edit. Accepts: (a) an absolute path to an app folder like `/Users/.../workspaces/myws/data/apps/habit-tracker`, (b) a workspace-relative path like `data/apps/habit-tracker` (resolved against the target workspace's root), or (c) a registered external repository name or UUID from `manage_repositories` (resolved to the repo's path). Omit to edit Lucidos source — only on an install launched from a Lucidos source checkout (see the system prompt's \"WHAT A CODING AGENT CAN EDIT ON THIS INSTALL\" section); elsewhere a `folder`-less call against this install is refused, though one carrying `workspace=\"<other>\"` still forwards to that workspace's engine. The engine selects the spawn kind automatically: Lucidos source ⇒ Lucidos-internal coding-agent thread (full /harden, Apply may restart the engine); `data/apps/<id>/` ⇒ app coding-agent thread (sparse-checkout worktree, Apply ff-merges to workspace git main, no engine restart, no /harden); registered external repo ⇒ external-repo coding-agent thread (no Apply gate). REFUSED: any `data/` path outside `data/apps/<id>/` (use chat tools + `lucidos` CLI for non-app data), subpaths inside an app (`data/apps/<id>/ui/`), file paths, the whole `data/`, unregistered git folders, non-git folders, `<workspace>/.lucidos/`, system paths."
                    },
                    // Temporary measure — registered in docs/temporary-measures.md
                    // § "`repo` → `folder` deprecated alias on `run_coding_agent`".
                    "repo": {
                        "type": "string",
                        "description": "DEPRECATED — use `folder` instead. Accepted for one release as an alias: equivalent to `folder = <resolved repo path>`. Passing both `folder` and `repo` is an error."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Target workspace basename (e.g. \"dev\", \"myws\"). Omit (or set to the current workspace name) for the default same-workspace spawn. When set to a different workspace, the tool POSTs to that workspace's engine and the coding-agent session lands there. Cross-workspace requires `relation=\"top\"` (child-thread auto-resume callbacks across workspaces are unsupported). The `folder` parameter then resolves in the TARGET workspace's repo registry, so make sure the repo is registered there."
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
                        "description": "Short descriptive title (3-6 words) for the spawned coding-agent thread. When provided, the system will not auto-generate a title. Set it: this is how you will refer to this child later, in your own prose and in the result of follow_up_child_thread, and it is what makes the thread list meaningful at a glance."
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
        ToolDefinition {
            name: tn::FOLLOW_UP_CHILD_THREAD.to_string(),
            description: FOLLOW_UP_CHILD_THREAD_DESCRIPTION.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "thread_id": {
                        "type": "string",
                        "description": "The child thread's uuid. Get it from the run_thread / run_coding_agent result, from a completion card, or from the threads tool's 'list' action with my_children: true. Titles are not accepted: they are not unique, and a fuzzy match would silently deliver to the wrong child."
                    },
                    "message": {
                        "type": "string",
                        "description": "What to tell the child. Write it as an instruction to the child, not as a description of the child: it lands in the child's conversation as a message from you."
                    },
                    "urgent": {
                        "type": "boolean",
                        "description": "Set true ONLY when the child must act on this instead of what it is doing now, typically a cancellation. It stops the child's current turn so it reads you immediately; whatever that turn was mid-way through is lost. Leave it out for an ordinary steer or an extra fact: the child then reads you when its current work reaches a natural break, which can take as long as the tool call it is inside. Default false."
                    }
                },
                "required": ["thread_id", "message"]
            }),
        },
    ]
}

/// Kept out of the `vec!` so the invariants below can assert on it by name
/// rather than by digging the tool back out of the list.
const FOLLOW_UP_CHILD_THREAD_DESCRIPTION: &str = concat!(
    "Send a follow-up message to a child thread YOU already spawned: redirect one that is ",
    "going the wrong way, hand it something a sibling learned, or tell a stalled one to ",
    "continue. Cheaper than spawning a replacement, and it does NOT consume a child slot, so ",
    "reviving an existing child is the right move when you are near the per-thread limit.\n\n",
    "Returns as soon as the message is on the child's timeline. It does NOT wait for the child ",
    "to finish: the child reports back the way it always does, by resuming this thread with a ",
    "completion when its turn ends. So issue the follow-up, then end your turn.\n\n",
    "You can only address your OWN direct children (not a sibling, not a grandchild, not a ",
    "thread someone else spawned). Refer to the child by TITLE in anything you write for the ",
    "user; the uuid is an addressing detail and never belongs in your prose.\n\n",
    "By default the message QUEUES: a child that is mid-turn reads it when its current work ",
    "reaches a natural break, and if it is inside a long tool call that can be many minutes. ",
    "That is the right default for a steer, because nothing in flight is thrown away. When the ",
    "child must act on your message INSTEAD of what it is doing (a cancellation, a stop, a ",
    "\"you are working from a wrong assumption\"), pass urgent: true and its current turn is ",
    "stopped so it reads you at once.\n\n",
    "Four things to know before you call it:\n",
    "- It RESOLVES ANY PENDING PERMISSION CARD on the child as superseded. If a human was ",
    "about to approve a tool call there, your redirect cancels that request.\n",
    "- If the child is parked on a question, your message is NOT an answer to it. The child ",
    "will not read the message until a human answers, and the result says so. urgent does not ",
    "change that: the child is blocked on a human, not on work.\n",
    "- A follow-up racing the child's own finish can produce a completion card for the turn ",
    "you interrupted. That does not mean the redirect failed; the redirected turn reports ",
    "separately when it ends.\n",
    "- On a CODEX child urgent changes nothing, because a Codex turn cannot read a queued ",
    "message at all until it ends, so every follow-up already stops the current turn. Pass ",
    "urgent by what you MEAN, not by which backend the child runs."
);

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

    /// The tool exposes NO caller-thread argument. The caller is
    /// `execute_tool`'s ambient `thread_id`, which the model cannot set, and
    /// that is the whole reason the authorization ladder is a real boundary
    /// rather than an accounting one: the model picks which child to address,
    /// never who it is.
    #[test]
    fn follow_up_tool_exposes_no_caller_thread_argument() {
        let tools = spawn_tools();
        let follow_up = tools
            .iter()
            .find(|tool| tool.name == tn::FOLLOW_UP_CHILD_THREAD)
            .expect("follow_up_child_thread tool must be registered");

        let props = follow_up.parameters["properties"]
            .as_object()
            .expect("properties object");
        let mut names: Vec<&str> = props.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["message", "thread_id", "urgent"],
            "the tool takes the child, the message and how hard to push, and nothing \
             else: any caller-thread argument would let the model claim to be someone else"
        );
        assert_eq!(
            follow_up.parameters["required"],
            serde_json::json!(["thread_id", "message"]),
            "urgent stays optional so omitting it yields the non-destructive default"
        );
    }

    /// `urgent` ends the child's current turn, so the model has to be told both
    /// what it costs and that the cost is real on only two of the three
    /// backends. A model that reads it as a free "please hurry" would spend it
    /// on every steer.
    #[test]
    fn follow_up_tool_states_what_urgent_costs_and_where_it_is_a_no_op() {
        let tools = spawn_tools();
        let follow_up = tools
            .iter()
            .find(|tool| tool.name == tn::FOLLOW_UP_CHILD_THREAD)
            .expect("follow_up_child_thread tool must be registered");

        let urgent = follow_up.parameters["properties"]["urgent"]["description"]
            .as_str()
            .expect("urgent description");
        assert!(
            urgent.contains("is lost"),
            "the model must know an urgent follow-up throws away in-flight work:\n{urgent}"
        );

        let d = &follow_up.description;
        assert!(
            d.contains("QUEUES"),
            "the default must be stated, or the model cannot tell what urgent changes:\n{d}"
        );
        assert!(
            d.contains("CODEX child urgent changes nothing"),
            "the Codex no-op is an honest asymmetry and must be stated, not hidden:\n{d}"
        );
    }

    /// Three side effects the model has to know about before it redirects a
    /// child, all user-visible and none of them obvious from the verb.
    #[test]
    fn follow_up_tool_description_warns_about_its_side_effects() {
        let tools = spawn_tools();
        let follow_up = tools
            .iter()
            .find(|tool| tool.name == tn::FOLLOW_UP_CHILD_THREAD)
            .expect("follow_up_child_thread tool must be registered");
        let d = &follow_up.description;

        assert!(
            d.contains("RESOLVES ANY PENDING PERMISSION CARD"),
            "a redirect resolves the child's pending permission cards as \
             superseded, which can cancel a request a human was about to \
             approve:\n{d}"
        );
        assert!(
            d.contains("parked on a question"),
            "a follow-up to a question-parked child is not read until a human \
             answers:\n{d}"
        );
        assert!(
            d.contains("does not mean the redirect failed"),
            "a follow-up racing the child's own finish can produce a card for \
             the turn it interrupted:\n{d}"
        );
        assert!(
            d.contains("does NOT consume a child slot"),
            "reviving an existing child is cheaper than spawning another:\n{d}"
        );
        assert!(
            d.contains("by TITLE"),
            "user-facing prose never names a thread by uuid:\n{d}"
        );
    }

    /// The title is the handle a parent uses to refer to a child later, so both
    /// spawn tools have to say so rather than calling it merely "Recommended".
    #[test]
    fn both_spawn_tools_say_the_title_is_how_you_refer_to_the_child() {
        for name in [tn::RUN_THREAD, tn::RUN_CODING_AGENT] {
            let tools = spawn_tools();
            let tool = tools
                .iter()
                .find(|t| t.name == name)
                .expect("spawn tool must be registered");
            let title = tool.parameters["properties"]["title"]["description"]
                .as_str()
                .expect("title description");
            assert!(
                title.contains("how you will refer to this child later"),
                "{name}'s title description must state what the title is FOR:\n{title}"
            );
        }
    }

    /// The `folder`-less form means "edit Lucidos itself", which only exists on
    /// an install launched from a source checkout. The description used to
    /// assert it unconditionally, and a packaged install's agent believed it —
    /// claiming to have read `crates/lucidos-engine/…`, spawning a session, and
    /// telling the user to Apply and rebuild. Both places that mention omitting
    /// `folder` must carry the precondition.
    #[test]
    fn omitting_folder_is_documented_as_needing_a_source_checkout() {
        let tools = spawn_tools();
        let run_coding_agent = tools
            .iter()
            .find(|tool| tool.name == tn::RUN_CODING_AGENT)
            .expect("run_coding_agent tool must be registered");

        assert!(
            run_coding_agent
                .description
                .contains("AVAILABLE ONLY on an install whose engine was launched from a Lucidos source checkout"),
            "the description's omit-`folder` bullet must state the precondition:\n{}",
            run_coding_agent.description
        );

        let folder = run_coding_agent.parameters["properties"]["folder"]["description"]
            .as_str()
            .expect("folder param must document itself");
        assert!(
            folder.contains("only on an install launched from a Lucidos source checkout"),
            "the `folder` param must state the precondition too — it is what the \
             model reads when deciding to omit it:\n{folder}"
        );
    }
}
