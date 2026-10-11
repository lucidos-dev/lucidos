//! `engine::pending_change_watch` against a real bus: a follow-up on a thread
//! whose change is already pending rebuilds the Changes frame.

use super::*;

#[tokio::test]
async fn a_follow_up_on_a_thread_with_a_pending_change_rebuilds_the_changes_frame() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let (shift_tx, mut shifts) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(crate::engine::pending_change_watch::watch(
        bus.subscribe(),
        move || {
            let shift_tx = shift_tx.clone();
            async move {
                shift_tx.send(()).unwrap();
            }
        },
    ));

    // The first turn ends with the change proposed, and the thread idle.
    let (_parent, thread_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, thread_id).await;
    emit_cc_idle(&bus, thread_id, true, None).await;
    // The watch's first sight of the proposed thread rebuilds once, and only once.
    let wait = std::time::Duration::from_secs(5);
    tokio::time::timeout(wait, shifts.recv())
        .await
        .expect("the first sight rebuilt the changes frame")
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(shifts.try_recv().is_err(), "the idle thread rebuilt twice");

    // The user sends a follow-up. Nothing about the change moves.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "revise the plan".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    tokio::time::timeout(wait, shifts.recv())
        .await
        .expect("the follow-up rebuilt the changes frame")
        .unwrap();
    // The frame it rebuilds now withholds Apply.
    let unsettled = crate::core::changes::unsettled_thread_ids(&pool, std::iter::once(thread_id))
        .await
        .unwrap();
    assert!(unsettled.contains(&thread_id));

    pool.close().await;
    teardown_test_db(&db_name).await;
}
