//! LLM-facing schemas for memory/context tools (correct_memory,
//! dismiss_from_context).


use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn correct_memory_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::CORRECT_MEMORY.to_string(),
            description: "Search for and correct wrong memories. Use when the user says a stored memory is wrong, inaccurate, or should be removed. Searches by keyword, then uses semantic similarity to the wrong_fact to only delete entries that actually express the wrong claim — other entries mentioning the same keyword are kept.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "search_query": {
                        "type": "string",
                        "description": "Keyword to find candidate memories (e.g., 'Acme Corp'). Broad is OK — semantic filtering narrows it down."
                    },
                    "wrong_fact": {
                        "type": "string",
                        "description": "The specific wrong claim to delete (e.g., 'User works at Acme Corp'). Only memories semantically similar to this are deleted."
                    },
                    "correction": {
                        "type": "string",
                        "description": "Optional corrected fact to store after deleting wrong memories. Omit to just delete without replacement."
                    }
                },
                "required": ["search_query", "wrong_fact"]
            }),
        },
    ]
}

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
