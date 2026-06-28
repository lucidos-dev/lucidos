//! LLM-facing schemas for notification tools (send/read notifications,
//! enable push). Handlers live in `engine::tools` + the scheduler push path.


use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

/// General-purpose notification tool, available in all contexts.
pub fn get_notification_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::SEND_NOTIFICATION.to_string(),
        description: "Send a notification to the user. The notification always lands in the inbox where the user can read it. How it ALSO surfaces depends on whether the user is actively viewing Lucidos: if the user has the app open/focused on ANY device, the OS push is suppressed on EVERY device and the active device(s) show an in-app toast instead — a device never gets both a push and a toast. An OS push only fires (to devices with push notifications enabled) when NO device is active — app backgrounded, screen off, or closed. Practical consequence: if the user is chatting with you right now, they are active, so they'll see this as an in-app toast on the device in front of them, NOT an OS push — do not tell them to 'check your device for the push'. The push only reaches their other, idle devices (or this one once they put it down). The push/in-app tap routing is controlled by `tap`, a structured `{kind, to?}` object: `{\"kind\":\"modal\"}` (default) opens the inbox modal so the user can read the message and decide what to do; `{\"kind\":\"none\"}` is passive (no destination, marks itself read on display); `{\"kind\":\"navigate\",\"to\":{...}}` delegates to the same router the `navigate_ui` tool uses — pass the same arg shape (target + target-specific sub-fields) so a tap can deep-link to any panel/app/file/thread/url reachable via navigate_ui. Use `app_id` to identify which app a notification is about — it powers the modal's \"Open <app>\" button.".to_string(),
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

// Reading / clearing the inbox (list / mark_read / mark_all_read) is the grouped
// `notifications` tool, built from the capability parity manifest
// (`crate::capability_manifest::build_llm_tool`). The retired flat
// `read_notifications` tool name still dispatches to that handler for back-compat
// (see `Domain::llm_aliases`). Only the *send* tool remains hand-written here,
// because its rich structured `tap` schema is a poor fit for the grouped union.

// Push notifications are no longer a standalone tool — enabling/declining them is
// `set_preference(key="push_notifications", value="enabled"|"declined")`, which
// keeps the [PUSH_NOTIFICATION_REQUEST] handshake. See core/preference_catalog.rs.
