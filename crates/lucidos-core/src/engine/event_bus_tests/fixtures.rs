async fn assert_children_counters(
    pool: &PgPool,
    parent_id: Uuid,
    expected_active: i32,
    expected_total: i32,
    msg: &str,
) {
    let (active, total): (i32, i32) = sqlx::query_as(
        "SELECT active_children_count, total_children_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(active, expected_active, "{} — active_children_count", msg);
    assert_eq!(total, expected_total, "{} — total_children_count", msg);
}

/// Read `blocking_descendant_count` for `thread_id`.
async fn read_blocking_descendant_count(pool: &PgPool, thread_id: Uuid) -> i32 {
    sqlx::query_scalar(
        "SELECT blocking_descendant_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Drain all SSE broadcasts the bus has accumulated since subscription,
/// returning a flat list of `(thread_id, event_type, aggregate)` tuples.
/// Caller filters by `thread_id` / aggregate presence to find specific
/// rebroadcasts. Ignores system events.
fn drain_aggregate_broadcasts(
    rx: &mut tokio::sync::broadcast::Receiver<EmittedEvent>,
) -> Vec<(Uuid, String, Option<crate::core::store::ThreadAggregate>)> {
    let mut out = Vec::new();
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::Thread {
            thread_id, event, ..
        } = &emitted.typed
        {
            out.push((
                *thread_id,
                event.event_type().to_string(),
                emitted.aggregate.clone(),
            ));
        }
    }
    out
}

async fn emit_cc_recovery_pair(bus: &EventBus, child_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::RecoveryAfterRestart,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            reason: Some(
                crate::engine::continuation_reason::ENGINE_RESTART_INTERRUPT_REASON.to_string(),
            ),
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}
