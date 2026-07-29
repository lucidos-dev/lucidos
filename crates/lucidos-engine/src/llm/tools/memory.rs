//! LLM-facing schema for the `dismiss_from_context` tool.
//!
//! Memory CORRECTION (correct / correct_by_id) is the grouped `memory` manifest
//! tool (built from `crate::capability_manifest`), so its schema lives there;
//! the flat `correct_memory` / `correct_memory_by_id` names stay wired as
//! back-compat aliases in `execute_tool`.


use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn dismiss_from_context_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::DISMISS_FROM_CONTEXT.to_string(),
            description: "Drop a prior tool result or child-thread completion from your future resume context. Use when you're done with that information and want to keep your context lean across long pipelines. Pass the event_id from prior history: tool blocks show `evt-<uuid>` as the tool_use_id, and ChildThreadCompleted blocks include an `event_id: <uuid>` line. Either form is accepted.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "event_id": {
                        "type": "string",
                        "description": "Event id of the ToolCalled or ChildThreadCompleted event to dismiss. Accepts either the bare UUID (hyphenated or simple) or the `evt-<uuid>` form rendered as tool_use_id in tool blocks."
                    }
                },
                "required": ["event_id"]
            }),
        },
    ]
}
