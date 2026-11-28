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

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/device-presence", post(update_device_presence))
}
