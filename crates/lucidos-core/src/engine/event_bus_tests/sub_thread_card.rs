use super::*;

fn question(tool_use_id: &str) -> ThreadEvent {
    ThreadEvent::UserQuestionAsked {
        tool_use_id: tool_use_id.into(),
        cc_session_id: String::new(),
        question: "A private question".into(),
        options: vec![],
        worktree_path: None,
        multi_select: false,
        owner_approval: None,
        permission_request: None,
    }
}

async fn relays(pool: &PgPool) -> Vec<(String, serde_json::Value)> {
    sqlx::query_as(
        "SELECT aggregate_id, payload->'data' FROM events
         WHERE event_type = 'SubThreadCardOpened' ORDER BY sequence",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn ancestors_only_with_one_copy_per_logical_opening() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let [root, parent, source, sibling, unrelated] = std::array::from_fn(|_| Uuid::new_v4());
    for (thread, ancestor) in [
        (root, None),
        (parent, Some(root)),
        (sibling, Some(parent)),
        (unrelated, None),
        (source, Some(parent)),
    ] {
        emit_thread_message(&bus, thread, ancestor, "start").await;
    }
    sqlx::query("UPDATE thread_summaries SET wake_mode = 'no-wake' WHERE thread_id = $1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();

    let mut live = bus.subscribe();
    let first = bus
        .emit(BusEvent::Thread {
            thread_id: source,
            event: question("one"),
            meta: EventMeta::NONE,
        })
        .await
        .unwrap()
        .unwrap();
    let _ = bus
        .emit(BusEvent::Thread {
            thread_id: source,
            event: question("one"),
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    let _ = bus
        .emit(BusEvent::Thread {
            thread_id: source,
            event: question("queued"),
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();

    let expected: Vec<_> = [(parent, source), (root, parent)]
        .into_iter()
        .map(|(ancestor, next_hop)| {
            (
                ancestor.to_string(),
                serde_json::json!({
                    "ancestor_thread_id": ancestor,
                    "child_thread_id": next_hop,
                    "source_thread_id": source,
                    "source_event_id": first.event_id,
                    "card_kind": "question",
                }),
            )
        })
        .collect();
    assert_eq!(relays(&pool).await, expected);
    let mut frames = Vec::new();
    while let Ok(frame) = live.try_recv() {
        if let BusEvent::System(event) = frame.typed {
            if event.event_type() == "SubThreadCardOpened" {
                assert!(frame.aggregate.is_none());
                assert!(frame.seq.unwrap() > first.seq);
                frames.push((event.aggregate_id(), event.to_payload()["data"].clone()));
            }
        }
    }
    assert_eq!(frames, expected);

    bus.emit(BusEvent::Thread {
        thread_id: source,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: "one".into(),
            answer: crate::engine::thread_events::AnswerKind::Canceled,
            quiet: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap()
    .unwrap();
    let promoted = bus
        .emit(BusEvent::Thread {
            thread_id: source,
            event: question("queued"),
            meta: EventMeta::NONE,
        })
        .await
        .unwrap()
        .unwrap();
    let rows = relays(&pool).await;
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[2].0, parent.to_string());
    assert_eq!(rows[3].0, root.to_string());
    for (_, relay) in &rows[2..] {
        assert_eq!(relay["source_event_id"], promoted.event_id.to_string());
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_failed_append_leaves_neither_the_card_nor_the_relays() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (root, source) = (Uuid::new_v4(), Uuid::new_v4());
    emit_thread_message(&bus, root, None, "parent").await;
    emit_thread_message(&bus, source, Some(root), "child").await;
    let mut live = bus.subscribe();

    let result = bus
        .emit_with_mutation(source, question("rollback"), |tx| {
            Box::pin(async move {
                let staged: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM events WHERE event_type = 'SubThreadCardOpened'",
                )
                .fetch_one(tx)
                .await?;
                assert_eq!(staged, 1);
                Err(sqlx::Error::Protocol("force rollback after staging".into()))
            })
        })
        .await;
    assert!(result.unwrap_err().to_string().contains("force rollback"));
    assert!(relays(&pool).await.is_empty());
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM events WHERE event_type = 'UserQuestionAsked')"
    )
    .fetch_one(&pool)
    .await
    .unwrap());
    assert!(live.try_recv().is_err());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn only_pending_permission_prompts_relay() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (root, source) = (Uuid::new_v4(), Uuid::new_v4());
    emit_thread_message(&bus, root, None, "parent").await;
    emit_thread_message(&bus, source, Some(root), "child").await;

    let request = |id: &str| ThreadEvent::McpPermissionRequested {
        request_id: id.into(),
        tool_use_id: "call".into(),
        server_id: "example".into(),
        server_name: "Example".into(),
        tool_name: "ExampleTool".into(),
        arguments_summary: "Private arguments".into(),
    };
    let resolution = ThreadEvent::McpPermissionResolved {
        request_id: "automatic".into(),
        allowed: false,
        reason: None,
        persist_scope: None,
    };
    bus.emit_atomically(
        source,
        vec![
            (request("automatic"), EventMeta::NONE),
            (resolution, EventMeta::NONE),
        ],
    )
    .await
    .unwrap()
    .unwrap();
    assert!(relays(&pool).await.is_empty());

    let posted = bus
        .emit(BusEvent::Thread {
            thread_id: source,
            event: request("owner"),
            meta: EventMeta::NONE,
        })
        .await
        .unwrap()
        .unwrap();
    let rows = relays(&pool).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1["card_kind"], "permission-request");
    assert_eq!(rows[0].1["source_event_id"], posted.event_id.to_string());
    assert_eq!(rows[0].1.as_object().unwrap().len(), 5);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn both_commit_orders_with_detach_use_the_edges_at_opening() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for card_first in [true, false] {
        let [root, parent, source] = std::array::from_fn(|_| Uuid::new_v4());
        emit_thread_message(&bus, root, None, "root").await;
        emit_thread_message(&bus, parent, Some(root), "parent").await;
        emit_thread_message(&bus, source, Some(parent), "source").await;
        let detach = ThreadEvent::ChildThreadDetached {
            child_thread_id: parent,
            child_thread_title: None,
        };
        let (first_thread, first_event, second_thread, second_event) = if card_first {
            (source, question("card"), root, detach)
        } else {
            (root, detach, source, question("card"))
        };
        let (locked, locks_held) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let first_bus = bus.clone();
        let first = tokio::spawn(async move {
            first_bus
                .emit_with_mutation(first_thread, first_event, |_| {
                    Box::pin(async move {
                        locked.send(()).unwrap();
                        released.await.unwrap();
                        Ok(())
                    })
                })
                .await
                .unwrap()
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), locks_held)
            .await
            .unwrap()
            .unwrap();
        let second_bus = bus.clone();
        let second = tokio::spawn(async move {
            second_bus
                .emit(BusEvent::Thread {
                    thread_id: second_thread,
                    event: second_event,
                    meta: EventMeta::NONE,
                })
                .await
                .unwrap()
        });
        wait_until_blocked_on_locks(&pool, 1).await;
        release.send(()).unwrap();
        assert!(first.await.unwrap());
        assert!(second.await.unwrap().is_some());
        let addressed: Vec<_> = relays(&pool)
            .await
            .into_iter()
            .filter(|(_, payload)| payload["source_thread_id"] == source.to_string())
            .map(|(addr, _)| addr)
            .collect();
        let mut expected = vec![parent.to_string()];
        if card_first {
            expected.push(root.to_string());
        }
        assert_eq!(addressed, expected);
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}
