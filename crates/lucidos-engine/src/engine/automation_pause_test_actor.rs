fn device(id: &str) -> Option<MessageOrigin> {
    Some(MessageOrigin::Device {
        device_id: id.into(),
    })
}
