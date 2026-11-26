async fn assert_active_children(pool: &PgPool, parent_id: Uuid, expected: i32, msg: &str) {
    let count: i32 = sqlx::query_scalar(
        "SELECT active_children_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, expected, "{}", msg);
}

async fn count_completion_cards(pool: &PgPool, parent_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted'",
    )
    .bind(parent_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn emit_response_canceled_with_cause(
    bus: &EventBus,
    thread_id: Uuid,
    cause: crate::engine::thread_events::CancelCause,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseCanceled {
            text: "partial work".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

fn drain_callbacks(rx: &mut tokio::sync::mpsc::UnboundedReceiver<ParentCallback>) -> usize {
    let mut n = 0;
    while rx.try_recv().is_ok() {
        n += 1;
    }
    n
}

async fn newest_completion_card(pool: &PgPool, parent_id: Uuid) -> (String, String) {
    sqlx::query_as(
        "SELECT payload->>'status', payload->>'summary' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(parent_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn emit_change_proposed_per_commit(
    bus: &EventBus,
    thread_id: Uuid,
    branch: &str,
    commit_sha: &str,
    description: Option<&str>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: String::new(),
            commit_sha: Some(commit_sha.into()),
            branch_name: branch.into(),
            description: description.map(Into::into),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            repo_root: String::new(),
            hardened: false,
            incomplete: false,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn force_branch_work(pool: &sqlx::PgPool, thread_id: Uuid) {
    sqlx::query(&format!(
        "UPDATE thread_summaries SET {} WHERE thread_id = $1",
        crate::engine::event_bus::branch_work_sql("TRUE")
    ))
    .bind(thread_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn read_attention_descendant_count(pool: &PgPool, thread_id: Uuid) -> i32 {
    sqlx::query_scalar(
        "SELECT attention_descendant_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn spawn_parent_with_idle_cc_child(bus: &EventBus, pool: &PgPool) -> (Uuid, Uuid) {
    let (parent_id, child_id) = spawn_parent_child(bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(bus, child_id).await;
    emit_cc_idle(bus, child_id, false, None).await;
    assert_eq!(
        read_blocking_descendant_count(pool, parent_id).await,
        0,
        "baseline: idle CC child contributes 0 to parent's blocking_descendant_count"
    );
    (parent_id, child_id)
}
