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
            description: "Drop a prior tool result or child-thread completion from your future resume context, to keep it lean across a long pipeline. Pass the event_id from history: a tool block shows `evt-<uuid>` as its tool_use_id, a ChildThreadCompleted block carries an `event_id: <uuid>` line, and either form is accepted.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "event_id": {
                        "type": "string",
                        "description": "Event id of the ToolCalled or ChildThreadCompleted event to dismiss. Bare UUID or the `evt-<uuid>` form."
                    }
                },
                "required": ["event_id"]
            }),
        },
    ]
}
