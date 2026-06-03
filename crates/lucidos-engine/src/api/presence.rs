//! `/device-presence` (`update_device_presence`): the frontend POSTs
//! `{ device_id, visible }` when ANY top-level Lucidos tab transitions
//! visible/hidden. Projects to `device_presence` and feeds the
//! candidate list the PresenceCheck protocol pings (see
//! `system-knowhow/notifications.md` §3). Per-thread focus is now
//! reported live in the pong rather than via a separate POST loop.

use super::*;

#[derive(Deserialize)]
pub struct DevicePresenceRequest {
    pub device_id: String,
    pub visible: bool,
}

pub(super) async fn update_device_presence(
    State(state): State<AppState>,
    Json(request): Json<DevicePresenceRequest>,
) -> Json<ApiResult> {
    if request.device_id.is_empty() {
        return ApiResult::err("device_id is required");
    }

    let event = if request.visible {
        crate::engine::event_bus::SystemEvent::DeviceVisible {
            device_id: request.device_id,
        }
    } else {
        crate::engine::event_bus::SystemEvent::DeviceHidden {
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
        Err(e) => ApiResult::err(format!("Failed to record device presence: {}", e)),
    }
}
