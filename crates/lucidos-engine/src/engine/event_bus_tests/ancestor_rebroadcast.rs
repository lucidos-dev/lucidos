use super::super::*;
use super::*;

// -----------------------------------------------------------------------
// Ancestor aggregate rebroadcast tests.
//
// When a descendant flips `is_blocking`, every ancestor's
// `blocking_descendant_count` moves. The DB column is updated by the
// recursive CTE in `propagate_blocking_change`, but consumers won't see
// the change until the ancestor's aggregate is rebroadcast over SSE.
// EventBus piggybacks on the existing `ChildrenCountChanged` transient
// (same shape as parent-callback's `send_children_count_event`) to carry
// the refreshed aggregate. Two invariants under test:
//   1. A flip event broadcasts the emitting thread's own aggregate AND
//      one aggregate per ancestor in the chain.
//   2. A non-flip activity event broadcasts only the emitting thread's
//      aggregate — no ancestor rebroadcast (would flood SSE on every
//      stream token).
// -----------------------------------------------------------------------
#[tokio::test]
async fn descendant_flip_broadcasts_ancestor_aggregate() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    // Build grandparent (chat) → parent (chat sub-thread) → CC grandchild,
    // each idled to a clean baseline (blocking_descendant_count = 0 on both
    // ancestors). Mirrors `three_level_tree_propagates_to_grandparent`.
    let grandparent_id = Uuid::new_v4();
    let parent_id = Uuid::new_v4();
    let grandchild_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: grandparent_id,
        event: ThreadEvent::MessageReceived {
            text: "root".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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
            text: "mid".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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

    // Settle both chat ancestors to Idle so the baseline is a clean 0.
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

    // Spawn the CC grandchild idle (not blocking).
    bus.emit(BusEvent::Thread {
        thread_id: grandchild_id,
        event: ThreadEvent::MessageReceived {
            text: "cc task".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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

    // Subscribe AFTER setup so we observe only the flipping event's broadcasts.
    let mut rx = bus.subscribe();

    // Flip the grandchild to Running. Expected broadcasts:
    //   1. The grandchild's own MessageReceived aggregate (existing behavior).
    //   2. Parent aggregate carrying blocking_descendant_count=1.
    //   3. Grandparent aggregate carrying blocking_descendant_count=1.
    bus.emit(BusEvent::Thread {
        thread_id: grandchild_id,
        event: ThreadEvent::MessageReceived {
            text: "wake".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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

    // 1. Emitting-thread aggregate: MessageReceived on grandchild with
    //    blocking_descendant_count=0 (the grandchild has no descendants).
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

    // 2. Parent aggregate via the ChildrenCountChanged piggyback. The frontend
    //    consumes the aggregate snapshot, not the event-type discriminator —
    //    matching `update_parent_after_child_terminal`'s existing shape.
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

    // 3. Grandparent aggregate — propagation walks the full chain.
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

    // Setup: parent + CC child already running (blocking). Get the child into
    // Running by following the idle → MessageReceived path so the parent's
    // count is already at 1 — meaning subsequent activity events on the child
    // leave is_blocking unchanged.
    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "ping".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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

    // Subscribe AFTER setup so we observe only the non-flipping event's
    // broadcasts.
    let mut rx = bus.subscribe();

    // CodingAgentTextStreamed bumps last_activity but keeps the child in
    // Running — is_blocking is unchanged, so no ancestor rebroadcast.
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentTextStreamed {
            text: "still working...".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let broadcasts = drain_aggregate_broadcasts(&mut rx);

    // The emitting thread's own aggregate should still broadcast.
    let own = broadcasts
        .iter()
        .find(|(tid, etype, _)| *tid == child_id && etype == "CodingAgentTextStreamed");
    assert!(
        own.is_some(),
        "expected CodingAgentTextStreamed broadcast for the emitting child"
    );

    // No ancestor rebroadcast — that would flood SSE on every stream token.
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
    // Per-token streaming events (TextStreamed, ThoughtStreamed,
    // CodingAgentTextStreamed) fire many times per CC turn. Their projection
    // arms only set status='running' (idempotent), so the blocking predicate
    // cannot flip. The projection short-circuits the before/after blocking
    // sample + ancestor walk for these events to keep the hot path cheap.
    //
    // This test verifies the short-circuit by:
    //  1. Setting up a parent + CC child already in a Running (blocking)
    //     state so the parent's blocking_descendant_count = 1.
    //  2. Emitting several CodingAgentTextStreamed events on the child.
    //  3. Asserting the parent's count stays at 1 (no spurious flip).
    //  4. Asserting NO ancestor rebroadcasts arrive, which is the
    //     observable signal that the projection skipped the sample +
    //     propagate path entirely.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    // Parent + CC child, get child into Running (blocking) so the parent's
    // count is already 1. Subsequent streaming events should leave it intact.
    let (parent_id, child_id) = spawn_parent_with_idle_cc_child(&bus, &pool).await;
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "ping".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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

    // Subscribe AFTER setup so we only observe the streaming events' broadcasts.
    let mut rx = bus.subscribe();

    // Emit a burst of per-token streaming events on the child — the projection
    // must short-circuit the blocking sample + ancestor walk for each.
    for i in 0..5 {
        bus.emit(BusEvent::Thread {
            thread_id: child_id,
            event: ThreadEvent::CodingAgentTextStreamed {
                text: format!("chunk {i}"),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }

    // Parent's count must remain at 1 — streaming events are pure activity,
    // they don't flip blocking.
    assert_eq!(
        read_blocking_descendant_count(&pool, parent_id).await,
        1,
        "per-token streaming events must not alter parent's blocking_descendant_count"
    );

    // No ancestor rebroadcast — the short-circuit means propagate_blocking_change
    // is never reached, so no `ChildrenCountChanged` aggregate fires for the parent.
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
