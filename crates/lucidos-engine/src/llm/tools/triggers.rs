//! LLM-facing schemas for trigger and trigger-group management tools.


use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

/// Shared JSON schema for the `cron` tool parameter.
/// When `nullable`, adds `null` as a valid type (for update_trigger to clear the schedule).
fn cron_schema(nullable: bool) -> serde_json::Value {
    let mut variants = vec![
        json!({ "type": "string" }),
        json!({ "type": "array", "items": { "type": "string" }, "minItems": 1 }),
    ];
    let desc = if nullable {
        variants.push(json!({ "type": "null" }));
        "Cron schedule(s) with 6 fields in USER'S LOCAL TIME: second minute hour day-of-month month day-of-week. Pass a single string for one schedule, an array of strings for multiple, or null to remove the cron schedule. Example: '0 0 8 * * *' for 8am daily."
    } else {
        "Cron schedule(s) with 6 fields in USER'S LOCAL TIME: second minute hour day-of-month month day-of-week. Pass a single string for one schedule, or an array of strings for multiple schedules (e.g., fire at both 8am and 8pm). Example: '0 0 8 * * *' for 8am daily, or ['0 0 8 * * *', '0 0 20 * * *'] for 8am and 8pm daily."
    };
    json!({ "description": desc, "oneOf": variants })
}

pub(super) fn trigger_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::CREATE_TRIGGER.to_string(),
            description: "Create a NEW trigger. Before calling this, list_triggers and prefer update_trigger for any tweak to an existing workflow (schedule, prompt, rename, pause, extra cron entry — append to the cron array even for one-shot extras). Recreating orphans the old trigger's run history. Two live triggers with identical names are a UX trap — name distinctly. Schedule-based (cron), event-based (on), or both. One trigger can subscribe to several events at once via the `on` array — each entry pairs an event type with an optional payload filter scoped to that event. Cron times in the USER'S LOCAL timezone. MUST set timezone first (set_timezone).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "A short, descriptive name for the trigger"
                    },
                    "run": {
                        "type": "object",
                        "description": "What to execute. Either { type: 'intent', intent: '...' } for LLM intents (one sentence in the user's voice — keep procedure out of the intent), or { type: 'script', path: 'name/run.py' } for scripts. If the LLM judges a procedure relevant at fire time, it calls `load_knowhow` itself — same as in chat. There is no per-trigger knowhow allow-list to configure here."
                    },
                    "cron": cron_schema(false),
                    "on": {
                        "description": "Event subscriptions the trigger fires on. Each entry is { event_type: 'X', condition?: {...} }. Condition is a payload filter scoped to that entry — it does NOT apply to other entries. Filter operators: $eq, $ne, $lt, $lte, $gt, $gte, $in. Example: [{ event_type: 'OuraSleepImported', condition: { sleep_score: { $lt: 70 } } }, { event_type: 'EmailReceived' }]. Shortcuts accepted: a single string 'X' becomes one no-condition entry; an array of strings becomes one no-condition entry each.",
                        "anyOf": [
                            { "type": "string" },
                            {
                                "type": "array",
                                "items": {
                                    "anyOf": [
                                        { "type": "string" },
                                        {
                                            "type": "object",
                                            "properties": {
                                                "event_type": { "type": "string" },
                                                "condition": { "type": "object" }
                                            },
                                            "required": ["event_type"]
                                        }
                                    ]
                                }
                            }
                        ]
                    },
                    "app_id": {
                        "type": "string",
                        "description": "Owning app directory name (e.g. 'trigger-workflow'). Set this when the trigger belongs to an app the user can open — notifications will deep-link to that app's UI. Omit for standalone triggers."
                    },
                    "go_to_review": {
                        "type": "boolean",
                        "description": "When true, threads spawned by this trigger surface in REVIEW on completion instead of going straight to ARCHIVE. Use for triggers whose output the user is meant to read — daily summaries, alerts, scheduled reports. Default false (silent execution, archive-only) suits most cron triggers."
                    },
                    "group_id": {
                        "type": "string",
                        "description": "ID of the *trigger group* this trigger belongs to. Pure organizational label — does not affect firing. Use list_trigger_groups to find an existing group (or create_trigger_group first). Omit for standalone triggers; they render under the 'Ungrouped' section in the panel."
                    }
                },
                "required": ["name", "run"]
            }),
        },
        ToolDefinition {
            name: tn::LIST_TRIGGERS.to_string(),
            description: "List all triggers the user has created. Shows trigger names, schedules, and what each trigger runs.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::UPDATE_TRIGGER.to_string(),
            description: "Update an existing trigger's name, schedule, event subscriptions, or run config. PREFER this over delete+create for any change to an existing workflow — the trigger_id stays stable so the run history stays linked. To add an extra firing time (including a temporary one-shot), append to the cron array; don't make a sibling trigger. To add or remove an event subscription, send the full replacement `on` array — partial edits aren't supported. Use list_triggers first to find the trigger ID. At least one field besides trigger_id must be provided.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "trigger_id": {
                        "type": "string",
                        "description": "UUID of the trigger to update"
                    },
                    "name": {
                        "type": "string",
                        "description": "New name for the trigger"
                    },
                    "run": {
                        "type": "object",
                        "description": "Change what to execute. { type: 'intent', intent: '...' } or { type: 'script', path: '...' }. If the LLM judges a procedure relevant at fire time, it calls `load_knowhow` itself — same as in chat. There is no per-trigger knowhow allow-list to configure here."
                    },
                    "cron": cron_schema(true),
                    "on": {
                        "description": "Full replacement for the event subscription list. Send the complete new set — there is no partial edit; append by re-sending the existing entries plus the new one(s). Same entry shapes as create_trigger: each entry is { event_type, condition? } (or a bare string as shorthand for no condition). Pass [] (or null) to clear all subscriptions, e.g. when switching the trigger to schedule-only.",
                        "anyOf": [
                            { "type": "null" },
                            { "type": "string" },
                            {
                                "type": "array",
                                "items": {
                                    "anyOf": [
                                        { "type": "string" },
                                        {
                                            "type": "object",
                                            "properties": {
                                                "event_type": { "type": "string" },
                                                "condition": { "type": "object" }
                                            },
                                            "required": ["event_type"]
                                        }
                                    ]
                                }
                            }
                        ]
                    },
                    "paused": {
                        "type": "boolean",
                        "description": "Pause/resume the trigger as part of a multi-field update. For pause/resume alone, prefer the dedicated pause_trigger / resume_trigger tools."
                    },
                    "app_id": {
                        "anyOf": [
                            { "type": "null" },
                            { "type": "string" }
                        ],
                        "description": "Owning app directory name (e.g. 'trigger-workflow'). Set to null to clear (e.g. trigger no longer belongs to any app)."
                    },
                    "go_to_review": {
                        "type": "boolean",
                        "description": "When true, future threads spawned by this trigger surface in REVIEW on completion instead of going straight to ARCHIVE. Setting this only affects new runs — already-completed threads are not retroactively re-routed."
                    },
                    "group_id": {
                        "anyOf": [
                            { "type": "null" },
                            { "type": "string" }
                        ],
                        "description": "Move the trigger into a *trigger group* (string id) or remove it from any group (null). Use list_trigger_groups to find the id."
                    }
                },
                "required": ["trigger_id"]
            }),
        },
        ToolDefinition {
            name: tn::DELETE_TRIGGER.to_string(),
            description: "Delete a trigger by its ID. Use list_triggers first to find the trigger ID.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "trigger_id": {
                        "type": "string",
                        "description": "UUID of the trigger to delete"
                    }
                },
                "required": ["trigger_id"]
            }),
        },
        ToolDefinition {
            name: tn::PAUSE_TRIGGER.to_string(),
            description: "Pause an existing trigger so it stops firing on its schedule and stops matching events. The trigger's config is preserved — use resume_trigger to re-enable it. Use list_triggers first to find the trigger ID.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "trigger_id": { "type": "string", "description": "UUID of the trigger to pause" }
                },
                "required": ["trigger_id"]
            }),
        },
        ToolDefinition {
            name: tn::RESUME_TRIGGER.to_string(),
            description: "Resume a previously paused trigger so it fires on its schedule and matches events again. Use list_triggers first to find the trigger ID.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "trigger_id": { "type": "string", "description": "UUID of the trigger to resume" }
                },
                "required": ["trigger_id"]
            }),
        },
        ToolDefinition {
            name: tn::LIST_TRIGGER_GROUPS.to_string(),
            description: "List trigger groups (user-visible folders that organize triggers in the panel). Pure label — groups don't fire or schedule anything; they just collect related triggers under a named, collapsible section. Returns id, name, order, and member_count for each. Use the id with create_trigger / update_trigger's `group_id` arg to assign triggers, or with the rename / reorder / delete tools to manage the group itself.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::CREATE_TRIGGER_GROUP.to_string(),
            description: "Create a trigger group — a named folder in the triggers panel that you can assign triggers to via `group_id`. Useful for surfacing emergent workflows (chains of triggers connected by emit_event → on_event) as one logical section. Group names are unique within the workspace (case-insensitive). The optional `order` controls panel sort position; omit to sink to the bottom.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Human-facing label shown as the section header." },
                    "order": { "type": "integer", "description": "Sort position in the panel (ascending). Omit to default to max(existing.order) + 10." }
                },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: tn::RENAME_TRIGGER_GROUP.to_string(),
            description: "Rename a trigger group. Fails if another group already uses the new name (case-insensitive).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "group_id": { "type": "string", "description": "UUID of the group to rename." },
                    "name": { "type": "string", "description": "New display name." }
                },
                "required": ["group_id", "name"]
            }),
        },
        ToolDefinition {
            name: tn::REORDER_TRIGGER_GROUPS.to_string(),
            description: "Atomic batch reorder of trigger groups. Pass an array of {id, order} entries; each entry whose order differs from the current value emits a TriggerGroupReordered event. Use this when nudging multiple groups at once — single-group reorders can also go through this tool with a one-element array.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ordering": {
                        "type": "array",
                        "description": "Array of { id, order } entries.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "order": { "type": "integer" }
                            },
                            "required": ["id", "order"]
                        }
                    }
                },
                "required": ["ordering"]
            }),
        },
        ToolDefinition {
            name: tn::DELETE_TRIGGER_GROUP.to_string(),
            description: "Delete a trigger group. Refuses with member_count + member_trigger_ids when the group still has triggers — move or delete those triggers first (update_trigger with group_id: null clears membership), then retry.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "group_id": { "type": "string", "description": "UUID of the group to delete." }
                },
                "required": ["group_id"]
            }),
        },
    ]
}
