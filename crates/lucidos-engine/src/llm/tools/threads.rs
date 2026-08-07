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
            description: "Start a new Lucidos thread for a non-code subtask (research, analysis, drafting). For code, use run_coding_agent. It runs its own agentic loop with full tool access, and with the default relation=\"child\" resumes THIS thread with its result, so you never poll. Spawn several in parallel and each reports back independently. Review what comes back before acting on it, and re-spawn with a refined prompt if it is incomplete.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The task for the new thread."
                    },
                    "title": {
                        "type": "string",
                        "description": "3-6 words. Set it: this is how you will refer to this child later, in your prose and in follow_up_child_thread results. Auto-generated if omitted."
                    },
                    "relation": {
                        "type": "string",
                        "enum": ["child", "top"],
                        "description": "'child' (default): this thread resumes with the result. 'top': fire-and-forget, for work the user will read rather than you. ('sub' is an alias.)"
                    }
                },
                "required": ["prompt"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_CODING_AGENT.to_string(),
            description: "Run a coding agent to edit source code: Lucidos itself, an installed app, or a registered external repo. ONLY when the user explicitly asks to modify code, never for workspace work the native tools cover. Default backend is Claude Code; set `coding_agent=\"codex\"` whenever the user asks for Codex. Spawning returns immediately and the ack is not a result: read the child's final response text for pass/fail before acting on it or reporting.\n\nPick `folder` first, asking which one if it is ambiguous. Omitting it means Lucidos itself, AVAILABLE ONLY on an install whose engine was launched from a Lucidos source checkout: the system prompt's \"WHAT A CODING AGENT CAN EDIT ON THIS INSTALL\" section says which install this is. load_knowhow('system-knowhow/coding-agent-events') for the folder table and cross-workspace.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The coding task. Be specific about which files to change and the outcome."
                    },
                    "coding_agent": {
                        "type": "string",
                        "enum": ["claude-code", "codex"],
                        "description": "Use \"codex\" whenever the user asks for Codex; omit for Claude Code."
                    },
                    "folder": {
                        "type": "string",
                        "description": "What to edit, which also picks the spawn kind: `data/apps/<id>`, or a registered repository name or UUID from `manage_repositories`. Omit to edit Lucidos source, which works only on an install launched from a Lucidos source checkout; a `folder`-less call against this install is otherwise refused, though one carrying `workspace` still forwards."
                    },
                    // Temporary measure — registered in docs/temporary-measures.md
                    // § "`repo` → `folder` deprecated alias on `run_coding_agent`".
                    "repo": {
                        "type": "string",
                        "description": "DEPRECATED, use `folder`. Passing both is an error."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Target workspace basename. Omit for this workspace. Another one requires `relation=\"top\"` and resolves `folder` there."
                    },
                    "allowed_tools": {
                        "type": "string",
                        "description": "Tools to auto-approve, comma-separated. Default Bash,Read,Edit,Write,Glob,Grep."
                    },
                    "model": {
                        "type": "string",
                        "description": "Model override. Omit for the backend default."
                    },
                    "append_system_prompt": {
                        "type": "string",
                        "description": "Extra instructions appended to the agent's system prompt."
                    },
                    "images": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Conversation images to forward, e.g. [\"thread:1\"], 1-based. Omit for the current message's; `[]` forwards none."
                    },
                    "title": {
                        "type": "string",
                        "description": "3-6 words. Set it: this is how you will refer to this child later, in your prose and in follow_up_child_thread results. Auto-generated if omitted."
                    },
                    "relation": {
                        "type": "string",
                        "enum": ["child", "top"],
                        "description": "'child' (default): this thread resumes when the session ends. 'top': fire-and-forget, and the only form a cross-workspace spawn accepts. ('sub' is an alias.)"
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
                        "description": "The child's uuid, from the spawn result, a completion card, or the threads tool's 'list' with my_children: true. Titles are not accepted: they are not unique."
                    },
                    "message": {
                        "type": "string",
                        "description": "What to tell the child. An instruction TO it, not a description OF it: it lands in the child's conversation as a message from you."
                    },
                    "urgent": {
                        "type": "boolean",
                        "description": "True ONLY when the child must act on this instead of what it is doing, typically a cancellation: whatever its turn was mid-way through is lost. Default false."
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
    "Send a follow-up message to a child thread YOU already spawned: redirect one going the ",
    "wrong way, hand it something a sibling learned, or tell a stalled one to continue. It ",
    "does NOT consume a child slot and the child gets a fresh ",
    "turn with the full tool set, so reviving one beats spawning another near the per-thread ",
    "limit. Refer to the child by TITLE in anything you write for the user. Returns as soon ",
    "as the message lands and does NOT wait for the child, so issue it and end your turn.\n\n",
    "By default the message QUEUES: a mid-turn child reads it at its next natural break, ",
    "which inside a long tool call can be many minutes, and nothing in flight is thrown away. ",
    "urgent: true stops the child's current turn instead. On a CODEX child urgent changes ",
    "nothing, since a Codex turn cannot read a queued message until it ends, so pass it by ",
    "what you MEAN rather than by which backend the child runs.\n\n",
    "Side effects invisible from the verb, all three in ",
    "`system-knowhow/coding-agent-events`: it RESOLVES ANY PENDING PERMISSION CARD on the ",
    "child as superseded; a follow-up racing the child's own finish can produce a completion ",
    "card for the turn you interrupted, which does not mean the redirect failed; and a child ",
    "parked on a question is blocked on a human, so your message is not an answer to it."
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
