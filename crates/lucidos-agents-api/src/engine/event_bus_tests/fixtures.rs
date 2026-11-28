async fn spawn_cc_parent_with_n_running_cc_children(bus: &EventBus, n: usize) -> (Uuid, Vec<Uuid>) {
    let parent_id = Uuid::new_v4();
    emit_cc_message_received(bus, parent_id, None, "parent").await;
    emit_cc_session_started(bus, parent_id).await;

    let children: Vec<Uuid> = (0..n).map(|_| Uuid::new_v4()).collect();
    for &child_id in &children {
        emit_cc_message_received(bus, child_id, Some(parent_id), "sub").await;
        emit_cc_session_started(bus, child_id).await;
    }
    (parent_id, children)
}
