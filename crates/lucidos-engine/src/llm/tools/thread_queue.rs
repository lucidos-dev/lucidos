//! LLM-facing schemas for Thread Queue policy tools.
//!
//! These are the chat-agent counterpart to the Thread Queue panel's
//! read/update surface. They keep policy tweaks inside typed engine tools
//! instead of making the agent discover local HTTP ports or write temp files.

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

fn cap_schema(description: &str) -> serde_json::Value {
    json!({
        "type": "integer",
        "minimum": 0,
        "description": description
    })
}

pub(super) fn thread_queue_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::LIST_THREAD_QUEUE.to_string(),
            description: "List the live Thread Queue and its active capacity policy. Returns compact JSON `{ entries, policy }`, where `entries` includes every occupant of the shared pool — queued/admitted background spawns AND user-initiated work (`kind: \"user-chat\"`) — and `policy` includes the current caps. Read-only. Use this before changing capacity so requests like \"double it\" are computed from the live policy, not hard-coded defaults.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::UPDATE_THREAD_QUEUE_POLICY.to_string(),
            description: "Update the Thread Queue capacity policy. Only fields you provide are changed; omitted fields keep the current live value. This emits the persisted `CapacityPolicyChanged` event, so the policy survives engine restarts. Use `list_thread_queue` first when the user asks for a relative change like \"double capacity\". `max_concurrent_total` is the shared ceiling for ALL threads — background spawns AND user-initiated work (user-initiated is prioritized but still counts and queues at the ceiling). Concurrency caps may be 0 to hold admission; `max_queued_per_trigger` must be at least 1. Keep `max_concurrent_per_trigger` at 1 unless the user explicitly wants one trigger to run multiple fires concurrently, because 1 preserves strict per-trigger FIFO.".to_string(),
            parameters: json!({
                "type": "object",
                "minProperties": 1,
                "properties": {
                    "max_concurrent_total": cap_schema("Maximum concurrently running threads across all kinds — background spawns AND user-initiated work."),
                    "max_concurrent_event_trigger": cap_schema("Maximum concurrently running event-trigger fires."),
                    "max_concurrent_cron": cap_schema("Maximum concurrently running cron-trigger fires."),
                    "max_concurrent_sub_thread": cap_schema("Maximum concurrently running agent-spawned sub-thread chats."),
                    "max_concurrent_coding_agent": cap_schema("Maximum concurrently running agent-spawned coding-agent threads."),
                    "max_concurrent_per_trigger": cap_schema("Maximum concurrent runs for one trigger. 1 preserves strict per-trigger FIFO."),
                    "max_queued_per_trigger": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Maximum queued backlog for one trigger before overflow handling applies."
                    },
                    "reserved_background": cap_schema("Slots background work can always reclaim ahead of user-initiated work, so user priority can't starve triggers/cron. 0 = pure user priority."),
                    "overflow": {
                        "type": "string",
                        "enum": ["drop-oldest", "pause-trigger"],
                        "description": "Overflow behavior when one trigger reaches `max_queued_per_trigger`."
                    }
                },
                "required": []
            }),
        },
    ]
}
