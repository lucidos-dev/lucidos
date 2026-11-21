//! LLM-facing schema for the `send_notification` tool. Its handler lives in
//! `engine::tools` + the scheduler push path.
//!
//! This module owns the SEND schema only. Reading / clearing the inbox is the
//! grouped `notifications` manifest tool, and enabling push is
//! `set_preference` on [`crate::core::prefs::PUSH_NOTIFICATIONS`]. See the
//! trailing comments below for both.

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

/// General-purpose notification tool, available in all contexts.
pub fn get_notification_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::SEND_NOTIFICATION.to_string(),
        description: "Send a notification; it always reaches the inbox. A shown toast, auto-read or silenced toasts mean OS push is suppressed on every device. Fullscreen can report not_shown, allowing push. Do not tell an active user to 'check your device for the push'.".to_string(),
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
                    "description": "App id from the Available Apps list, driving the modal's \"Open <app>\" button. Independent of `tap`, so pass it whenever the notification is about an app."
                },
                "tap": {
                    "type": "object",
                    // The thread id clause is terse because the always-loaded
                    // budget bills it on every request. It earns its 12 chars:
                    // a model that guesses `current` here writes a dead deep
                    // link to a phone. See
                    // docs/plans/2026-09-18-notification-tap-thread-id-is-a-uuid.md
                    "description": "Where a tap lands. Omitted: the `event_id` you passed, else the inbox card. `{\"kind\":\"modal\"}` forces the card. `{\"kind\":\"navigate\",\"to\":{…}}` takes the same router args `navigate_ui` does, e.g. `{\"kind\":\"navigate\",\"to\":{\"target\":\"thread\",\"id\":\"current\"}}`, or a uuid. An app tap takes `fragment`, landing on the one item the notification is about."
                },
                "event_id": {
                    "type": "string",
                    "description": "Event uuid inside the originating thread. A tap then lands on it and pulses it. A trigger's source event id works only if that event is in a thread; a domain event is refused."
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

// Push notifications are no longer a standalone tool. Enabling or declining them
// is `set_preference` on `prefs::PUSH_NOTIFICATIONS` with `enabled` or
// `declined`, which keeps the [PUSH_NOTIFICATION_REQUEST] handshake.
