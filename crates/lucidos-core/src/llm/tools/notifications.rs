use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

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
                    // INVARIANT: The thread id clause is terse because the always-loaded
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
