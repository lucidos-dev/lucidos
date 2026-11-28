use uuid::Uuid;

/// INVARIANT: The failure sibling of `ensure_terminator_emitted`, sharing its gate.
/// Emits an ANCHORED `ResponseFailed` unless a terminator already landed for
/// `request_event_id`.
///
/// Every error exit that happens once an exchange exists funnels through
/// here, which is what makes `process_message_with_steps` the single owner of
/// a turn's terminator. Its callers used to each bolt on their own
/// `ResponseFailed` with `EventMeta::NONE`, because the pre-loop exits emitted
/// nothing. Anchorless, those copies walked straight past the gate. So every
/// in-loop failure wrote the event twice and fired every subscribed trigger
/// twice. Anchor the emit and the gate does its job.
pub async fn ensure_failure_terminator_emitted(
    bus: &crate::engine::event_bus::EventBus,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    request_event_id: Uuid,
    channel: Option<crate::engine::thread_events::EventChannel>,
    error: &str,
) {
    ensure_agent_failure_terminator_emitted(
        bus,
        pool,
        thread_id,
        request_event_id,
        channel,
        error,
        None,
    )
    .await;
}

pub async fn ensure_run_failure_terminator_emitted(
    bus: &crate::engine::event_bus::EventBus,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    request_event_id: Uuid,
    channel: Option<crate::engine::thread_events::EventChannel>,
    error: &(dyn std::error::Error + Send + Sync + 'static),
) {
    if crate::engine::start_failure::awaits_owner(error) {
        return;
    }
    ensure_agent_failure_terminator_emitted(
        bus,
        pool,
        thread_id,
        request_event_id,
        channel,
        &error.to_string(),
        crate::engine::start_failure::agent_exit_of(error),
    )
    .await;
}

async fn ensure_agent_failure_terminator_emitted(
    bus: &crate::engine::event_bus::EventBus,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    request_event_id: Uuid,
    channel: Option<crate::engine::thread_events::EventChannel>,
    error: &str,
    agent_exit: Option<crate::engine::thread_events::CodingAgentExit>,
) {
    if crate::engine::thread_events::has_terminator_for(pool, thread_id, request_event_id).await {
        return;
    }

    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                error: error.to_string(),
                spend_cap: crate::llm::spend_cap::SpendCapNotice::from_agent_text(
                    "Provider", error,
                ),
                agent_exit,
                provider_error: None,
            },
            meta: crate::engine::thread_events::EventMeta {
                request_event_id: Some(request_event_id),
                channel,
                ..crate::engine::thread_events::EventMeta::NONE
            },
        },
        "[AgenticLoop] ResponseFailed (turn failed outside the loop)",
    )
    .await;
}
