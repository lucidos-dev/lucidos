use super::*;

fn projection_free_event(thread_id: Uuid) -> BusEvent {
    BusEvent::Thread {
        thread_id,
        event: ThreadEvent::WorkingUnderstandingWritten {
            document: "the job so far".into(),
        },
        meta: EventMeta::NONE,
    }
}

async fn created_of(pool: &PgPool, event_id: Uuid) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT created FROM events WHERE id = $1")
        .bind(event_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_later_emit_on_a_thread_cannot_commit_before_an_earlier_one() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;

    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM thread_summaries WHERE thread_id = $1 FOR UPDATE")
        .bind(thread_id)
        .execute(&mut *holder)
        .await
        .unwrap();

    let a = tokio::spawn({
        let bus = bus.clone();
        async move {
            bus.emit(thread_message(thread_id, None, "earlier"))
                .await
                .unwrap()
        }
    });
    wait_until_blocked_on_locks(&pool, 1).await;

    let mut b = tokio::spawn({
        let bus = bus.clone();
        async move { bus.emit(projection_free_event(thread_id)).await.unwrap() }
    });
    let b_overtook_a = tokio::select! {
        _ = &mut b => true,
        _ = wait_until_blocked_on_locks(&pool, 2) => false,
    };
    assert!(
        !b_overtook_a,
        "a later emit committed while an earlier emit on the same thread was still \
         uncommitted, so a reader could see its sequence and skip the earlier one"
    );

    holder.commit().await.unwrap();
    let a = a.await.unwrap().expect("A is persisted");
    let b = b.await.unwrap().expect("B is persisted");
    assert!(
        a.seq < b.seq,
        "A queued first, so it must hold the lower sequence"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn created_is_stamped_after_the_append_lock_is_granted() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;

    let mut holder = pool.begin().await.unwrap();
    EventBus::lock_thread_append(&mut holder, thread_id)
        .await
        .unwrap();
    let queued = tokio::spawn({
        let bus = bus.clone();
        async move { bus.emit(projection_free_event(thread_id)).await.unwrap() }
    });
    wait_until_blocked_on_locks(&pool, 1).await;

    let released_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    holder.commit().await.unwrap();
    let queued = queued.await.unwrap().expect("the queued emit is persisted");
    assert!(
        created_of(&pool, queued.event_id).await >= released_at,
        "created predates the lock grant, so it was stamped at transaction start"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_historical_replay_on_a_thread_waits_for_an_emit_in_flight() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    emit_thread_message(&bus, thread_id, None, "start").await;

    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM thread_summaries WHERE thread_id = $1 FOR UPDATE")
        .bind(thread_id)
        .execute(&mut *holder)
        .await
        .unwrap();
    let a = tokio::spawn({
        let bus = bus.clone();
        async move {
            bus.emit(thread_message(thread_id, None, "earlier"))
                .await
                .unwrap()
        }
    });
    wait_until_blocked_on_locks(&pool, 1).await;

    let mut replay = tokio::spawn({
        let bus = bus.clone();
        async move {
            bus.replay_historical_event(HistoricalReplay {
                event_id: Uuid::new_v4(),
                aggregate: "thread",
                aggregate_id: &thread_id.to_string(),
                event_type: "WorkingUnderstandingWritten",
                payload: &serde_json::json!({ "document": "backfilled" }),
                thread_id: Some(thread_id),
                created: None,
                broadcast: false,
            })
            .await
            .unwrap()
        }
    });
    let replay_overtook_a = tokio::select! {
        _ = &mut replay => true,
        _ = wait_until_blocked_on_locks(&pool, 2) => false,
    };
    assert!(
        !replay_overtook_a,
        "a replayed row committed past an emit still in flight on its thread"
    );

    holder.commit().await.unwrap();
    let a = a.await.unwrap().expect("A is persisted");
    let replayed = replay.await.unwrap().expect("the replay inserted its row");
    assert!(a.seq < replayed, "the replay queued behind A");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn concurrent_emits_on_a_family_never_deadlock() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    emit_thread_message(&bus, parent, None, "parent").await;
    let mut children = Vec::new();
    for i in 0..4 {
        let child = Uuid::new_v4();
        emit_thread_message(&bus, child, Some(parent), &format!("child {i}")).await;
        children.push(child);
    }

    let mut emits = Vec::new();
    for child in &children {
        emits.push(BusEvent::Thread {
            thread_id: *child,
            event: ThreadEvent::ResponseGenerated {
                text: "done".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
            },
            meta: EventMeta::NONE,
        });
        emits.push(projection_free_event(*child));
        emits.push(projection_free_event(parent));
    }
    let results = futures::future::join_all(emits.into_iter().map(|event| {
        let bus = bus.clone();
        tokio::spawn(async move { bus.emit(event).await.map_err(|e| e.to_string()) })
    }))
    .await;
    for result in results {
        result
            .unwrap()
            .expect("no emit may fail, a deadlock included");
    }

    assert_children_counters(&pool, parent, 0, 4, "every child finished").await;
    assert_eq!(
        count_completion_cards(&pool, parent).await,
        4,
        "a completion card failed after its child's commit"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_teardown_aborting_a_whole_family_at_once_never_deadlocks() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for round in 0..12 {
        let top = Uuid::new_v4();
        emit_thread_message(&bus, top, None, "top").await;
        let mut running = Vec::new();
        for _ in 0..3 {
            let parent = Uuid::new_v4();
            emit_thread_message(&bus, parent, Some(top), "parent").await;
            running.push(parent);
            for _ in 0..2 {
                let child = Uuid::new_v4();
                emit_thread_message(&bus, child, Some(parent), "child").await;
                running.push(child);
            }
        }

        let results = futures::future::join_all(running.iter().map(|thread_id| {
            let bus = bus.clone();
            let thread_id = *thread_id;
            tokio::spawn(async move {
                bus.emit(BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ResponseAborted {
                        text: String::new(),
                        images: vec![],
                        model: None,
                        reasoning_effort: None,
                        cause: crate::engine::thread_events::AbortCause::EngineShutdown,
                    },
                    meta: EventMeta {
                        actor: Some(crate::engine::thread_events::device_actor("d")),
                        ..EventMeta::NONE
                    },
                })
                .await
                .map_err(|e| e.to_string())
            })
        }))
        .await;
        for result in results {
            result
                .unwrap()
                .unwrap_or_else(|e| panic!("round {round}: a teardown abort failed: {e}"));
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}
