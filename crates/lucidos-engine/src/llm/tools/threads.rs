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

/// The `reasoning_effort` values `run_coding_agent` advertises: the union of
/// what the two backends offer, in ladder order.
///
/// A union rather than one backend's list, because the tool serves both and the
/// two vocabularies can drift apart at any time. Advertising the union keeps a
/// value that IS valid somewhere out of the schema's way, and per-backend
/// validation at the spawn boundary rejects it by name for the backend that
/// does not take it. That is the same split the `model` argument uses: the
/// schema describes the surface, the spawn enforces the backend.
fn coding_agent_effort_vocabulary() -> Vec<&'static str> {
    let offered = |agent| {
        crate::runtime::coding_agent_reasoning_effort_options(agent)
            .iter()
            .map(|o| o.value.as_str())
            .collect::<Vec<_>>()
    };
    let claude = offered(crate::runtime::CodingAgent::ClaudeCode);
    let codex = offered(crate::runtime::CodingAgent::Codex);
    crate::llm::EFFORT_LADDER
        .iter()
        .copied()
        .filter(|tier| claude.contains(tier) || codex.contains(tier))
        .collect()
}

/// The `model` values `run_coding_agent` advertises: the union of what the two
/// backends offer, each backend's own picker order, Claude Code first then
/// Codex.
///
/// Models are not a scale, so unlike [`coding_agent_effort_vocabulary`] this
/// does not sort. It preserves picker order and drops any value both backends
/// offer, `default` today: it means "that backend's own default" in each, so
/// it is one entry, not two identical ones. A future overlap dedupes the same
/// way, with no new special case to remember.
///
/// The union is deliberately loose: it says nothing about which id belongs to
/// which backend, so the schema still admits a Codex id paired with
/// `coding_agent: "claude-code"`. `validate_coding_agent_model` refuses that
/// at the spawn, which is correct and unchanged. The enum's job is narrower:
/// kill the id that exists in NO backend picker, the whole class of mistake a
/// chat-picker id used here is.
fn coding_agent_model_vocabulary() -> Vec<&'static str> {
    let mut seen = std::collections::HashSet::new();
    [
        crate::runtime::CodingAgent::ClaudeCode,
        crate::runtime::CodingAgent::Codex,
    ]
    .into_iter()
    .flat_map(crate::runtime::coding_agent_model_options)
    .filter_map(|option| {
        let value = option.value.as_str();
        seen.insert(value).then_some(value)
    })
    .collect()
}

pub(super) fn spawn_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::RUN_THREAD.to_string(),
            description: "Start a new Lucidos thread for a non-code subtask (research, analysis, drafting). For code, use run_coding_agent. It runs its own agentic loop with full tool access, and with the default relation=\"child\" resumes THIS thread with its result, so you never poll. Spawn several in parallel and each reports back independently. Review what comes back before acting on it, and re-spawn if it is incomplete.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The task for the new thread."
                    },
                    "title": {
                        "type": "string",
                        "description": "3-6 words, and this is how you will refer to this child later in your prose. Auto-generated if omitted."
                    },
                    "relation": {
                        "type": "string",
                        "enum": ["child", "top"],
                        "description": "'child' (default): this thread resumes with the result. 'top': fire-and-forget, for work the user will read rather than you. ('sub' is an alias.)"
                    },
                    "model": {
                        "type": "string",
                        "description": "Chat model id, e.g. 'claude-sonnet-5'. Omit for the account default; a mechanical subtask wants a cheaper one and less thinking."
                    },
                    "reasoning_effort": {
                        "type": "string",
                        "enum": crate::llm::EFFORT_LADDER,
                        "description": "Thinking budget. Omit for the account default."
                    }
                },
                "required": ["prompt"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_CODING_AGENT.to_string(),
            description: "Run a coding agent to edit source code: Lucidos itself, an installed app, or a registered repo. ONLY when the user explicitly asks to modify code, never for workspace work the native tools cover. Spawning returns immediately and the ack is NOT a result: read the child's final response text for pass/fail before acting on it or reporting.\n\nPick `folder` first, asking which one if it is ambiguous. Omitting it means Lucidos itself, AVAILABLE ONLY on an install whose engine was launched from a Lucidos source checkout: the system prompt's \"WHAT A CODING AGENT CAN EDIT ON THIS INSTALL\" section says which. load_knowhow('system-knowhow/coding-agent-events') for the folder table and cross-workspace.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The coding task. Name the files and the outcome."
                    },
                    "coding_agent": {
                        "type": "string",
                        "enum": ["claude-code", "codex"],
                        "description": "Use \"codex\" when the user asks for Codex; omit for Claude Code."
                    },
                    "folder": {
                        "type": "string",
                        "description": "What to edit, which also picks the spawn kind: `data/apps/<id>`, or a registered repository name or UUID from `manage_repositories`. Omit to edit Lucidos source, which works only on an install launched from a Lucidos source checkout; otherwise a `folder`-less call is refused unless it carries `workspace`."
                    },
                    // Temporary measure — registered in docs/temporary-measures.md
                    // § "`repo` → `folder` deprecated alias on `run_coding_agent`".
                    "repo": {
                        "type": "string",
                        "description": "DEPRECATED alias of `folder`; both is an error"
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Target workspace basename. Omit for this one. Another needs `relation=\"top\"` and resolves `folder` there."
                    },
                    // No `allowed_tools` and no `append_system_prompt` here, on
                    // purpose. Both were declared for a year and neither ever
                    // reached the spawn: `run_session` overwrites the allowlist
                    // from the user's `cc-allowed-tools` file and composes the
                    // system prompt itself. Wiring them was the wrong repair.
                    // The allowlist is the USER'S permission surface, so a caller
                    // that could widen its own child's allowlist would be a
                    // boundary hole, and the system prompt is engine-owned.
                    // Declaring neither is what makes the schema honest.
                    "model": {
                        "type": "string",
                        "enum": coding_agent_model_vocabulary(),
                        "description": "An id the chosen backend does not offer is REFUSED, never swapped for the default. Omit to inherit. A one-file edit wants Sonnet, not Opus."
                    },
                    "reasoning_effort": {
                        "type": "string",
                        "enum": coding_agent_effort_vocabulary(),
                        "description": "Thinking budget. Omit for the backend default."
                    },
                    "images": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Conversation images to forward, e.g. [\"thread:1\"], 1-based. Omit for the current message's; `[]` none."
                    },
                    "title": {
                        "type": "string",
                        "description": "3-6 words, and how you will refer to this child later in your prose. Auto-generated if omitted."
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
                        "description": "The child's uuid, from the spawn result, a completion card, or the threads tool's 'list' with my_children: true. Titles are not accepted."
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
    "Send a follow-up to a child thread YOU already spawned: redirect one going the wrong ",
    "way, hand it something a sibling learned, or tell a stalled one to continue. It does ",
    "NOT consume a child slot, so reviving one beats spawning another near the per-thread ",
    "limit. Refer to the child by TITLE in anything you write for the user. Returns as soon ",
    "as the message lands, so issue it and end your turn.\n\n",
    "By default the message QUEUES: a mid-turn child reads it at its next natural break, ",
    "which inside a long tool call can be many minutes, and nothing in flight is thrown away. ",
    "urgent: true stops the child's current turn instead. On a CODEX child urgent changes ",
    "nothing, since a Codex turn cannot read a queued message until it ends.\n\n",
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

    /// Both route pins are optional and neither may join `required`: a caller
    /// that names only one still inherits the account default for the other.
    /// The effort enum is the ladder itself rather than a copy, so the schema
    /// cannot offer a tier `validate_trigger_reasoning_effort` would reject.
    #[test]
    fn run_thread_offers_optional_model_and_reasoning_effort_pins() {
        let tools = spawn_tools();
        let run_thread = tools
            .iter()
            .find(|tool| tool.name == tn::RUN_THREAD)
            .expect("run_thread tool must be registered");

        assert_eq!(
            run_thread.parameters["properties"]["model"]["type"],
            "string"
        );
        let effort = &run_thread.parameters["properties"]["reasoning_effort"];
        assert_eq!(effort["type"], "string");
        assert_eq!(
            effort["enum"],
            serde_json::json!(crate::llm::EFFORT_LADDER),
            "the offered tiers must be the unified ladder"
        );
        assert_eq!(
            run_thread.parameters["required"],
            serde_json::json!(["prompt"]),
            "only the prompt is required: both route pins fall back to the \
             account preference"
        );
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

    /// The schema may only advertise an argument the spawn path actually reads.
    ///
    /// `model`, `allowed_tools` and `append_system_prompt` were declared here
    /// and read by nothing: `ThreadQueueRequest::CodingAgent` had no field for
    /// any of them, so every session ran on the `cc-settings.json` default while
    /// the tool result came back a plain success. An agent told to match model
    /// to task obeyed, reported "I picked Sonnet", and ran Opus every time.
    ///
    /// So the rule this test pins is not "declare these three": it is that the
    /// declared set and the honoured set are the SAME set. `model` and
    /// `reasoning_effort` are now honoured, so they are declared. The other two
    /// are owned elsewhere and refused at the boundary, so they are not.
    #[test]
    fn the_coding_agent_schema_declares_only_arguments_the_spawn_honours() {
        let tools = spawn_tools();
        let run_coding_agent = tools
            .iter()
            .find(|tool| tool.name == tn::RUN_CODING_AGENT)
            .expect("run_coding_agent tool must be registered");
        let props = run_coding_agent.parameters["properties"]
            .as_object()
            .expect("properties object");

        for honoured in ["model", "reasoning_effort"] {
            assert!(
                props.contains_key(honoured),
                "`{honoured}` reaches the spawn, so the model must be told it exists"
            );
        }
        for owned_elsewhere in ["allowed_tools", "append_system_prompt"] {
            assert!(
                !props.contains_key(owned_elsewhere),
                "`{owned_elsewhere}` is overwritten by run_session and can never take \
                 effect; declaring it invites a caller to pass it and be silently ignored"
            );
        }
    }

    /// The effort enum is built from the backends' own pickers, so a tier added
    /// to either `cc_menu_options.json` or `codex_menu_options.json` shows up
    /// here without anyone remembering to edit this file. A hand-copied list is
    /// how the CC alias tables went stale before (three `wontfix` rows in the
    /// tracker say so).
    #[test]
    fn the_effort_enum_is_the_union_of_what_the_backends_offer() {
        let vocabulary = coding_agent_effort_vocabulary();
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for option in crate::runtime::coding_agent_reasoning_effort_options(agent) {
                assert!(
                    vocabulary.contains(&option.value.as_str()),
                    "{} offers '{}' but the schema does not advertise it",
                    agent.as_str(),
                    option.value
                );
            }
        }
        // Ladder order, not insertion order: the model reads this as a scale.
        let ladder: Vec<&str> = crate::llm::EFFORT_LADDER
            .iter()
            .copied()
            .filter(|t| vocabulary.contains(t))
            .collect();
        assert_eq!(vocabulary, ladder, "the enum must read low-to-high");
    }

    /// The model enum is built from the backends' own pickers. An id added to
    /// either `cc_menu_options.json` or `codex_menu_options.json` shows up
    /// here without anyone remembering to edit this file. Mirrors
    /// `the_effort_enum_is_the_union_of_what_the_backends_offer` above.
    #[test]
    fn the_model_enum_is_the_union_of_what_the_backends_offer() {
        let vocabulary = coding_agent_model_vocabulary();
        for agent in [
            crate::runtime::CodingAgent::ClaudeCode,
            crate::runtime::CodingAgent::Codex,
        ] {
            for option in crate::runtime::coding_agent_model_options(agent) {
                assert!(
                    vocabulary.contains(&option.value.as_str()),
                    "{} offers '{}' but the schema does not advertise it",
                    agent.as_str(),
                    option.value
                );
            }
        }
        assert_eq!(
            vocabulary.iter().filter(|v| **v == "default").count(),
            1,
            "both backends offer 'default'; the enum must list it once"
        );
    }
}
