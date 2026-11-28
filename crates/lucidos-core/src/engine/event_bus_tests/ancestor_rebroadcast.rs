use super::super::*;
use super::*;

#[tokio::test]
async fn descendant_flip_broadcasts_ancestor_aggregate() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let grandparent_id = Uuid::new_v4();
    let parent_id = Uuid::new_v4();
    let grandchild_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: grandparent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "root".into(),
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
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "mid".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(grandparent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    for tid in [parent_id, grandparent_id] {
        bus.emit(BusEvent::Thread {
            thread_id: tid,
            event: ThreadEvent::ResponseGenerated {
                text: "settled".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }

    bus.emit(BusEvent::Thread {
        thread_id: grandchild_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "cc task".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
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
    emit_cc_session_started(&bus, grandchild_id).await;
    emit_cc_idle(&bus, grandchild_id, false, None).await;

    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        0,
        "baseline: parent count 0 before flip"
    );
    assert_eq!(
        read_blocking_descendant_count(&pool, grandparent_id).await,
        0,
        "baseline: grandparent count 0 before flip"
    );

    let mut rx = bus.subscribe();

    bus.emit(BusEvent::Thread {
        thread_id: grandchild_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "wake".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
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

    let broadcasts = drain_aggregate_broadcasts(&mut rx);

    let own = broadcasts.iter().find(|(tid, etype, agg)| {
        *tid == grandchild_id && etype == "MessageReceived" && agg.is_some()
    });
    assert!(
        own.is_some(),
        "expected MessageReceived broadcast for grandchild with aggregate; got {:?}",
        broadcasts
            .iter()
            .map(|(t, e, a)| (t, e.as_str(), a.is_some()))
            .collect::<Vec<_>>()
    );

    let parent_agg = broadcasts.iter().find_map(|(tid, etype, agg)| {
        if *tid == parent_id && etype == "ChildrenCountChanged" {
            agg.clone()
        } else {
            None
        }
    });
    let parent_agg = parent_agg.expect(
        "parent aggregate must be rebroadcast after descendant flip — frontend's \
         meta.blockingDescendantCount stays stale otherwise",
    );
    assert_eq!(
        parent_agg.blocking_descendant_count, 1,
        "parent aggregate must carry the bumped count"
    );

    let grandparent_agg = broadcasts.iter().find_map(|(tid, etype, agg)| {
        if *tid == grandparent_id && etype == "ChildrenCountChanged" {
            agg.clone()
        } else {
            None
        }
    });
    let grandparent_agg = grandparent_agg.expect(
        "grandparent aggregate must also be rebroadcast — propagation walks every ancestor",
    );
    assert_eq!(
        grandparent_agg.blocking_descendant_count, 1,
        "grandparent aggregate must carry the bumped count"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn no_blocking_flip_does_not_rebroadcast_ancestors() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "ping".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
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
    assert_eq!(read_blocking_descendant_count(&pool, parent_id).await, 1);

    let mut rx = bus.subscribe();

    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentTextStreamed {
            text: "still working...".into(),
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            parent_tool_use_id: None,
            progress_note: false,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let broadcasts = drain_aggregate_broadcasts(&mut rx);

    let own = broadcasts
        .iter()
        .find(|(tid, etype, _)| *tid == child_id && etype == "CodingAgentTextStreamed");
    assert!(
        own.is_some(),
        "expected CodingAgentTextStreamed broadcast for the emitting child"
    );

    let ancestor_broadcasts: Vec<_> = broadcasts
        .iter()
        .filter(|(tid, _, _)| *tid == parent_id)
        .collect();
    assert!(
        ancestor_broadcasts.is_empty(),
        "non-flipping activity events must not rebroadcast ancestor aggregates; got {:?}",
        ancestor_broadcasts
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn per_token_streaming_does_not_sample_blocking() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "ping".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
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
    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        1,
        "baseline: parent count is 1 after CC child enters Running"
    );

    let mut rx = bus.subscribe();

    for i in 0..5 {
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::CodingAgentTextStreamed {
                text: format!("chunk {i}"),
                coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
                parent_tool_use_id: None,
                progress_note: false,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        1,
        "per-token streaming events must not alter parent's blocking_descendant_count"
    );

    let broadcasts = drain_aggregate_broadcasts(&mut rx);
    let ancestor_broadcasts: Vec<_> = broadcasts
        .iter()
        .filter(|(tid, _, _)| *tid == parent_id)
        .collect();
    assert!(
        ancestor_broadcasts.is_empty(),
        "per-token streaming must short-circuit the ancestor walk entirely; \
         got {} ancestor broadcasts: {:?}",
        ancestor_broadcasts.len(),
        ancestor_broadcasts
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
