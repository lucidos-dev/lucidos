//! LLM-facing schema for the `send_notification` tool. Its handler lives in
//! `engine::tools` + the scheduler push path.
//!
//! This module owns the SEND schema only. Reading / clearing the inbox is the
//! grouped `notifications` manifest tool, and enabling push is
//! `set_preference(key="push_notifications", …)`. See the trailing comments
//! below for both.

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

/// General-purpose notification tool, available in all contexts.
pub fn get_notification_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::SEND_NOTIFICATION.to_string(),
        description: "Send a notification to the user. It always lands in the inbox. How it ALSO surfaces depends on whether any device is active: with the app open and focused on ANY device the OS push is suppressed on EVERY device and the active ones show an in-app toast, so a device never gets both. A push fires only when NO device is active. So a user chatting with you right now will see a toast, never a push: do not tell them to 'check your device for the push'. Every notification is openable.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short title, in the user's language."
                },
                "message": {
                    "type": "string",
                    "description": "Body text, in the user's language."
                },
                "app_id": {
                    "type": "string",
                    "description": "Optional app id from the Available Apps list, driving the modal's \"Open <app>\" button. Independent of `tap`, so pass it whenever the notification is about an app."
                },
                "tap": {
                    "type": "object",
                    "description": "Where a tap lands. `{\"kind\":\"modal\"}` (the default) opens the inbox modal, for anything info-only. `{\"kind\":\"navigate\",\"to\":{…}}` deep-links through the same router and arg shape `navigate_ui` takes, e.g. `{\"kind\":\"navigate\",\"to\":{\"target\":\"thread\",\"id\":\"<uuid>\"}}`."
                },
                "event_id": {
                    "type": "string",
                    "description": "Optional event uuid inside the originating thread; the modal's \"Open thread\" button and a `navigate` tap both scroll to it and pulse it on land. Pass the source event id from a trigger's `## Triggering Event` block. Ignored with no linked thread."
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
