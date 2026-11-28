use crate::engine::thread_events::CodingAgent;
use crate::engine::thread_events::WakeMode;
use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

fn coding_agent_effort_vocabulary() -> Vec<&'static str> {
    let offered = |agent| {
        crate::llm::coding_agent_models::coding_agent_reasoning_effort_options(agent)
            .iter()
            .map(|o| o.value.as_str())
            .collect::<Vec<_>>()
    };
    let claude = offered(crate::engine::thread_events::CodingAgent::ClaudeCode);
    let codex = offered(crate::engine::thread_events::CodingAgent::Codex);
    crate::llm::EFFORT_LADDER
        .iter()
        .copied()
        .filter(|tier| claude.contains(tier) || codex.contains(tier))
        .collect()
}

fn wake_mode_property() -> serde_json::Value {
    json!({
        "type": "string",
        "enum": WakeMode::ALL.map(WakeMode::as_str),
        "description": "relation='child' only. Default 'no-wake' from the home thread, else 'wake' (FAN-OUT)."
    })
}

fn hook_args_property() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": { "type": "string" },
        "description": "Declared event hook argument names and their selected values in the target workspace."
    })
}

pub(super) fn spawn_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::RUN_THREAD.to_string(),
            description: "Start a thread for research, analysis, or drafting with full tool access. For code use run_coding_agent. Parallel child threads each report here. Wait for the report and review it before acting. Re-spawn if incomplete.".to_string(),
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
                        "description": "'child' (default): it reports to this thread. 'top': no report, cheaper when you will not act on the result. ('sub' is an alias.)"
                    },
                    "wake_mode": wake_mode_property(),
                    "hook_args": hook_args_property(),
                    "model": {
                        "type": "string",
                        "description": "Model id; with reasoning_effort, a Lucidos Agent rung, else REFUSED. Omit to run on this turn's model. Routine: lower; hard: higher."
                    },
                    "reasoning_effort": {
                        "type": "string",
                        "enum": crate::llm::EFFORT_LADDER,
                        "description": "Thinking budget. Omit to keep this turn's, or the new model's default if you name another model."
                    }
                },
                "required": ["prompt"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_CODING_AGENT.to_string(),
            description: "Edit Lucidos, an app or a registered repo at the user's explicit request. Use native tools for workspace work. Spawns acknowledge immediately. Read the child's final reply before acting or reporting.\n\nPick `folder` first. Ask if ambiguous. Omit it to edit Lucidos, AVAILABLE ONLY on an install whose engine was launched from a Lucidos source checkout. See WHAT A CODING AGENT CAN EDIT ON THIS INSTALL. load_knowhow('system-knowhow/coding-agent-events') for folder and cross-workspace rules.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The coding task. Name the files and the outcome. Its folder already sets defaults for review, merging and hardening: mention one only to ask for something other than that default."
                    },
                    "coding_agent": {
                        "type": "string",
                        "enum": CodingAgent::ALL.map(CodingAgent::as_str),
                        "description": "Omit for the installed agent, Claude Code first."
                    },
                    "folder": {
                        "type": "string",
                        "description": "What to edit, which also picks the spawn kind: `data/apps/<id>`, or a registered repository name or UUID from `manage_repositories`. Omit to edit Lucidos source, which works only on an install launched from a Lucidos source checkout."
                    },
                    "workspace": {
                        "type": "string",
                        "description": "Basename of a workspace on THIS install; omit for this one. Needs `relation=\"top\"`; resolves `folder` there. Another gateway: `lucidos spawn-thread --to <ws>@<pairing>`."
                    },
                    // INVARIANT: No `allowed_tools` and no `append_system_prompt` here, on
                    // purpose. Both were declared for a year and neither ever
                    // reached the spawn: `run_session` overwrites the allowlist
                    // from the user's `cc-allowed-tools` file and composes the
                    // system prompt itself. Wiring them was the wrong repair.
                    // The allowlist is the USER'S permission surface, so a caller
                    // that could widen its own child's allowlist would be a
                    // boundary hole, and the system prompt is engine-owned.
                    // Declaring neither is what makes the schema honest.
                    // A string, not an enum. Claude Code's list is discovered per
                    // workspace, and an enum billed every picker row on every
                    // chat request. The spawn validates against the live picker.
                    "model": {
                        "type": "string",
                        "description": "A picker id or a thread's reported model. With reasoning_effort, one rung of the backend's intelligence ladder; an off-ladder pick is REFUSED, listing the rungs. Omit both if unsure. Settled: lower; hard: higher."
                    },
                    "reasoning_effort": {
                        "type": "string",
                        "enum": coding_agent_effort_vocabulary(),
                        "description": "The rung's other half."
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
                        "description": "'child' (default): it reports to this thread when its session ends. 'top': no report, cheaper when you will not act on the result; the only form a cross-workspace spawn accepts. ('sub' is an alias.)"
                    },
                    "wake_mode": wake_mode_property(),
                    "hook_args": hook_args_property()
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

const FOLLOW_UP_CHILD_THREAD_DESCRIPTION: &str = concat!(
    "Send a follow-up to a child thread YOU already spawned: redirect one going the wrong ",
    "way, hand it something a sibling learned, or tell a stalled one to continue. The ",
    "child cap never refuses it, so reviving a child beats spawning another. ",
    "Refer to the child by TITLE in anything the user reads. Returns at once, ",
    "even when held, so issue it and end your turn. A no-wake child's ",
    "report never wakes you.\n\n",
    "By default the message QUEUES: a mid-turn child reads it at its next natural break, ",
    "which inside a long tool call can be many minutes, and nothing in flight is thrown away. ",
    "urgent: true stops the child's current turn instead, on either backend. Codex with ",
    "app-server steer support takes ordinary messages mid-turn; older app-servers and ",
    "exec interrupt for every follow-up.\n\n",
    "Side effects invisible from the verb, all three in ",
    "`system-knowhow/coding-agent-events`: a child's PENDING PERMISSION CARD HOLDS your ",
    "message until a human acts; a follow-up racing the child's own finish can produce a completion ",
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

    #[test]
    fn both_spawn_tools_declare_the_wake_mode() {
        let tools = spawn_tools();
        for name in [tn::RUN_THREAD, tn::RUN_CODING_AGENT] {
            let tool = tools.iter().find(|t| t.name == name).expect(name);
            let wake_mode = &tool.parameters["properties"]["wake_mode"];
            assert_eq!(
                wake_mode["enum"],
                serde_json::json!(WakeMode::ALL.map(WakeMode::as_str)),
                "{name}"
            );
            let described = wake_mode["description"].as_str().unwrap();
            assert!(
                described.contains("'no-wake' from the home thread"),
                "{name}"
            );
        }
    }

    #[test]
    fn both_spawn_tools_advise_top_when_no_report_is_needed() {
        let tools = spawn_tools();
        for name in [tn::RUN_THREAD, tn::RUN_CODING_AGENT] {
            let tool = tools.iter().find(|t| t.name == name).expect(name);
            let described = tool.parameters["properties"]["relation"]["description"]
                .as_str()
                .unwrap();
            assert!(
                described.contains("you will not act on the result"),
                "{name}: {described}"
            );
        }
    }

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
            d.contains("app-server steer support") && d.contains("exec interrupt"),
            "the model must know when Codex preserves the turn and when it interrupts:\n{d}"
        );
    }

    #[test]
    fn follow_up_tool_description_warns_about_its_side_effects() {
        let tools = spawn_tools();
        let follow_up = tools
            .iter()
            .find(|tool| tool.name == tn::FOLLOW_UP_CHILD_THREAD)
            .expect("follow_up_child_thread tool must be registered");
        let d = &follow_up.description;

        assert!(
            d.contains("PENDING PERMISSION CARD HOLDS your message"),
            "a redirect into a child with a pending permission card is held \
             until a human resolves the card (ADR 0256), so the parent must \
             not expect it to land at once:\n{d}"
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
            d.contains("child cap never refuses it"),
            "a follow-up is never refused at the child cap, so reviving an \
             existing child beats spawning another:\n{d}"
        );
        assert!(
            d.contains("by TITLE"),
            "user-facing prose never names a thread by uuid:\n{d}"
        );
    }

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

    #[test]
    fn the_effort_enum_is_the_union_of_what_the_backends_offer() {
        let vocabulary = coding_agent_effort_vocabulary();
        for agent in [
            crate::engine::thread_events::CodingAgent::ClaudeCode,
            crate::engine::thread_events::CodingAgent::Codex,
        ] {
            for option in
                crate::llm::coding_agent_models::coding_agent_reasoning_effort_options(agent)
            {
                assert!(
                    vocabulary.contains(&option.value.as_str()),
                    "{} offers '{}' but the schema does not advertise it",
                    agent.as_str(),
                    option.value
                );
            }
        }
        let ladder: Vec<&str> = crate::llm::EFFORT_LADDER
            .iter()
            .copied()
            .filter(|t| vocabulary.contains(t))
            .collect();
        assert_eq!(vocabulary, ladder, "the enum must read low-to-high");
    }

    #[test]
    fn the_model_argument_is_a_string_the_spawn_checks() {
        let tool = spawn_tools()
            .into_iter()
            .find(|t| t.name == tn::RUN_CODING_AGENT)
            .expect("run_coding_agent");
        let model = &tool.parameters["properties"]["model"];
        assert_eq!(model["type"], "string");
        assert!(model.get("enum").is_none(), "got {model}");
    }
}
