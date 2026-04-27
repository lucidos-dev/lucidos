//! Thread presence/focus endpoints.
//!
//! The frontend POSTs `{ device_id, thread_id, focused }` whenever its focus
//! state changes (focus, blur, visibility change, heartbeat, beforeunload).
//! The handler converts it into the appropriate `SystemEvent` and emits
//! through EventBus, which then projects to the `thread_presence` table.

use super::*;

#[derive(Deserialize)]
pub struct PresenceRequest {
    pub device_id: String,
    pub thread_id: Uuid,
    pub focused: bool,
}

pub(super) async fn update_presence(
    State(state): State<AppState>,
    Json(request): Json<PresenceRequest>,
) -> Json<ApiResult> {
    if request.device_id.is_empty() {
        return ApiResult::err("device_id is required");
    }

    let event = if request.focused {
        crate::engine::event_bus::SystemEvent::ThreadFocused {
            thread_id: request.thread_id,
            device_id: request.device_id,
        }
    } else {
        crate::engine::event_bus::SystemEvent::ThreadUnfocused {
            thread_id: request.thread_id,
            device_id: request.device_id,
        }
    };

    match state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::System(event))
        .await
    {
        Ok(_) => ApiResult::ok(),
        Err(e) => ApiResult::err(format!("Failed to record presence: {}", e)),
    }
}
