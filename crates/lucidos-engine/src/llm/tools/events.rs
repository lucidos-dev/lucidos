//! LLM-facing schemas for domain-event tools (emit_event, query_events,
//! count_events).


use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn event_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::EMIT_EVENT.to_string(),
            description: "Emit a domain event to the event store. Use this to record task outcomes (e.g., GoogleDocEdited, OuraDataImported). Events are immutable facts — past tense, append-only.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "event_type": {
                        "type": "string",
                        "description": "Event type in PascalCase past tense (e.g., GoogleDocEdited, DataImported)"
                    },
                    "payload": {
                        "type": "object",
                        "description": "Event payload — REQUIRED. Include enough context to understand what happened. Example: {\"documentId\": \"abc\", \"title\": \"My Doc\", \"summary\": \"Changed title color to black\", \"operations\": 3}",
                        "properties": {
                            "summary": {
                                "type": "string",
                                "description": "Human-readable description of what happened"
                            }
                        },
                        "required": ["summary"]
                    }
                },
                "required": ["event_type", "payload"]
            }),
        },
        ToolDefinition {
            name: tn::QUERY_EVENTS.to_string(),
            description: "Query domain events from the event store. Returns events in reverse chronological order (newest first), wrapped as `{events, total_matching, returned, byte_size, truncated, hint?}`. Honours a 128 KB default byte budget — when `truncated:true`, follow the `hint` (narrow by aggregate_id or shorten the time window). For high-volume sweeps over many event types, call `count_events` first to size the windows before drilling — do NOT enumerate all events of all types into context. **Cumulative discipline:** the engine caps each call (`limit` ≤ 200, `byte_limit` ≤ 512 KB) but does NOT track per-turn totals; calling this 5+ times in one assistant turn can still blow the next turn's prompt budget. Treat 3 calls per turn as a soft ceiling — `count_events` first, then drill the narrowest type(s).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "event_type": {
                        "type": "string",
                        "description": "Filter by event type (e.g., CheckboxToggled, DataImported). Omit to query all event types — but prefer a specific filter on busy workspaces."
                    },
                    "since": {
                        "type": "string",
                        "description": "Only return events after this ISO 8601 / RFC 3339 timestamp (e.g., 2026-03-01T00:00:00Z)"
                    },
                    "until": {
                        "type": "string",
                        "description": "Only return events before this ISO 8601 / RFC 3339 timestamp"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of events to return (1-200, default 50). The default 50 is right for sampling-style sweeps; raise only when you intend full enumeration of a small event type. The hard cap is 200."
                    },
                    "byte_limit": {
                        "type": "integer",
                        "description": "Per-call byte budget for the compact-JSON response (1024-524288, default 131072 = 128 KB; hard cap 512 KB). When exceeded the response is truncated with `truncated:true` and a `hint`. Don't bump this without narrowing the query first (aggregate_id or tighter since/until)."
                    }
                }
            }),
        },
        ToolDefinition {
            name: tn::COUNT_EVENTS.to_string(),
            description: "Count events matching filters without materialising payloads. With `event_type`, returns `{count, byte_total}` for that type. Without `event_type`, returns `{by_type: [{event_type, count, byte_total}, ...], total_count, total_byte_total}` sorted by count desc — the 'what's noisy in this window' view. Call this BEFORE `query_events` on busy windows so you can budget which types to drill into and which to skip (e.g. types under the friction-signal threshold). `byte_total` is the raw payload byte sum and is a reliable proxy for the token cost of a corresponding `query_events` call.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "event_type": {
                        "type": "string",
                        "description": "Filter by event type. Omit to get a per-type breakdown across all event types."
                    },
                    "since": {
                        "type": "string",
                        "description": "Only count events after this ISO 8601 / RFC 3339 timestamp"
                    },
                    "until": {
                        "type": "string",
                        "description": "Only count events before this ISO 8601 / RFC 3339 timestamp"
                    }
                }
            }),
        },
    ]
}
