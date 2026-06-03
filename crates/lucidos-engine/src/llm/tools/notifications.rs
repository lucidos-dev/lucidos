//! LLM-facing schemas for notification tools (send/read notifications,
//! enable push). Handlers live in `engine::tools` + the scheduler push path.


use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

/// General-purpose notification tool, available in all contexts.
pub fn get_notification_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::SEND_NOTIFICATION.to_string(),
        description: "Send a notification to the user. The notification always lands in the inbox where the user can read it. When push notifications are enabled the user is also pushed on their devices. The push/in-app tap routing is controlled by `tap`, a structured `{kind, to?}` object: `{\"kind\":\"modal\"}` (default) opens the inbox modal so the user can read the message and decide what to do; `{\"kind\":\"none\"}` is passive (no destination, marks itself read on display); `{\"kind\":\"navigate\",\"to\":{...}}` delegates to the same router the `navigate_ui` tool uses — pass the same arg shape (target + target-specific sub-fields) so a tap can deep-link to any panel/app/file/thread/url reachable via navigate_ui. Use `app_id` to identify which app a notification is about — it powers the modal's \"Open <app>\" button.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short notification title (in the user's language)."
                },
                "message": {
                    "type": "string",
                    "description": "Notification body text (in the user's language)."
                },
                "app_id": {
                    "type": "string",
                    "description": "Optional id of an app from the Available Apps list. Identifies which app this notification is about — drives the modal's \"Open <app>\" button. Independent of `tap` — pass it whenever the notification is *about* an app even if the tap navigates elsewhere."
                },
                "tap": {
                    "type": "object",
                    "description": "Where a tap lands — structured discriminated union. Three kinds:\n\
                        • `{\"kind\":\"modal\"}` (default): opens the inbox modal so the user reads the message and chooses what to do.\n\
                        • `{\"kind\":\"none\"}`: passive — no destination; the row marks itself read on in-app toast display or OS push tap (which just launches the PWA, no deep-link). Use for info-only notifications that need no follow-up.\n\
                        • `{\"kind\":\"navigate\",\"to\":{...}}`: deep-link to a UI surface via the same arg shape `navigate_ui` accepts. Examples: `{\"kind\":\"navigate\",\"to\":{\"target\":\"thread\",\"id\":\"<thread-uuid>\",\"event_id\":\"<event-uuid>\"}}` to land on the originating thread and scroll-and-pulse the source event; `{\"kind\":\"navigate\",\"to\":{\"target\":\"app\",\"app_id\":\"habit-tracker\"}}` for a CTA into an app; `{\"kind\":\"navigate\",\"to\":{\"target\":\"url\",\"url\":\"https://...\"}}` for an external link.\n\
                        Required sub-fields per target follow the `navigate_ui` contract."
                },
                "event_id": {
                    "type": "string",
                    "description": "Optional UUID of a specific event inside the originating thread to deep-link to. When set, both the inbox modal's \"Open thread\" button AND a `{\"kind\":\"navigate\",\"to\":{\"target\":\"thread\",...}}` tap scroll and briefly pulse this event on land. Pass the source event id from the trigger's `## Triggering Event` block (e.g. the UserQuestionAsked or CodingAgentPermissionRequest row) so the user jumps straight to what they need to answer. Ignored when the notification has no linked thread."
                }
            },
            "required": ["title", "message"]
        }),
    }
}

/// Tool for reading notifications (past notifications, unread count, etc.).
pub fn get_read_notifications_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::READ_NOTIFICATIONS.to_string(),
        description: "Read notifications from the notification inbox. Use this to check what notifications have been sent (including task error notifications), see unread counts, or review notification history.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "enum": ["unread", "all"],
                    "description": "Filter: 'unread' for only unread notifications, 'all' for all. Default: 'unread'."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of notifications to return (1-50, default 20)."
                }
            }
        }),
    }
}

pub(super) fn enable_push_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::ENABLE_PUSH_NOTIFICATIONS.to_string(),
            description: "Enable or decline browser push notifications. Call with enabled=true if the user wants OS-level alerts for triggered tasks, or enabled=false if they decline.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "enabled": {
                        "type": "boolean",
                        "description": "true to enable push notifications, false to decline (won't ask again)"
                    }
                },
                "required": ["enabled"]
            }),
        },
    ]
}
