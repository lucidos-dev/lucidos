use super::{AbortCause, CancelCause, EventChannel, EventMeta, MessageOrigin, ThreadEvent};

/// Single grep target for "where can a response be canceled?". `meta.actor`
/// should be `Some(_)` whenever the cancel originated at the chat-cancel
/// HTTP handler — the handler resolves the actor and stamps it on the
/// per-thread `ThreadHandle.cancel_actor` slot, which the agentic loop's
/// cancel arms drain via `take_cancel_actor` and merge into the emitted
/// meta (see `meta_with_cancel_actor` in `agentic_loop.rs`). Engine-
/// internal cancels (shutdown, restart) leave the slot empty on purpose
/// because they pre-emit their own boundary events with explicit actor
/// upstream. The idempotency gate below only suppresses the loop's
/// follow-up emit when the pre-emitted terminator shares the same
/// `request_event_id` (e.g. `/api/v1/restart` via `abort_in_flight_for_restart`);
/// for shutdown the pre-emit carries no `request_event_id`, so the loop's
/// actor-less `ResponseCanceled` still lands and the UI relies on
/// `ResponseAborted` taking precedence in status derivation (see
/// `shutdown_active_threads`' own docstring).
///
/// Idempotent against pre-emitted terminators: if `meta.request_event_id` is
/// set and a `ResponseGenerated`/`ResponseCanceled`/`ResponseAborted`/
/// `ResponseFailed` already exists for it, this is a no-op. Covers the
/// `/api/v1/restart` race — `abort_in_flight_for_restart` pre-emits
/// `ResponseAborted{actor: device}` for the in-flight chat thread, then
/// cancels its token; the agentic loop's cancel branch fires moments later
/// and would otherwise stack a phantom `ResponseCanceled{UserStop}` boundary
/// on top of the "You — Restarted" panel.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_response_canceled(
    bus: &crate::engine::event_bus::EventBus,
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    cause: CancelCause,
    text: String,
    images: Vec<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    meta: EventMeta,
    log_tag: &str,
) {
    if let Some(req_id) = meta.request_event_id {
        if has_terminator_for(pool, thread_id, req_id).await {
            crate::log!(
                "[{}] Skipping ResponseCanceled — terminator already exists for request {} on thread {}",
                log_tag,
                req_id,
                thread_id
            );
            return;
        }
    }
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseCanceled {
                text,
                images,
                model,
                reasoning_effort,
                cause,
            },
            meta,
        },
        log_tag,
    )
    .await;
}

/// True iff a terminator (`ResponseGenerated`/`ResponseCanceled`/
/// `ResponseAborted`/`ResponseFailed`) already exists in the events table
/// for `(thread_id, request_event_id)`. Shared by `emit_response_canceled`
/// (skip-emit gate) and `agentic_loop::ensure_terminator_emitted`
/// (post-loop fallback gate). Fails open on DB error: returns `false` so
/// the caller still emits — a phantom terminal is much better than leaving
/// the UI stuck on "running" because the DB hiccupped.
pub(crate) async fn has_terminator_for(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
    request_event_id: uuid::Uuid,
) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(\
            SELECT 1 FROM events \
            WHERE aggregate_id = $1 \
              AND event_type = ANY($3::text[]) \
              AND payload->>'request_event_id' = $2\
        )",
    )
    .bind(thread_id.to_string())
    .bind(request_event_id.to_string())
    .bind(ThreadEvent::TERMINATOR_EVENT_TYPES)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!(
            "[ThreadEvents] has_terminator_for query failed for request {} on thread {}: {}",
            request_event_id,
            thread_id,
            e
        );
        false
    })
}

/// Single grep target for "where can a response be aborted?". `meta.actor`
/// covers both pure system kills (`None` or `Some(MessageOrigin::system())`)
/// and engine-deliberate terminations carrying `Engine{reason}` for the chip.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_response_aborted(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: uuid::Uuid,
    cause: AbortCause,
    text: String,
    images: Vec<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    meta: EventMeta,
    log_tag: &str,
) {
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseAborted {
                text,
                images,
                model,
                reasoning_effort,
                cause,
            },
            meta,
        },
        log_tag,
    )
    .await;
}

/// Fire-and-forget `ContinuationRequested` emit with consistent meta shape.
/// Two callers today: `agent_question`'s answered-after-idle resume and
/// the safety-net watchdog auto-recovery. The third source —
/// `/api/v1/threads/{id}/continue` — emits inline because it propagates
/// `Err` to an HTTP 500. `reason` must be one of the `*_REASON`
/// constants in `agent_recovery`; new reasons must opt in to
/// `continue_should_open_resume_exchange` deliberately.
pub(crate) async fn emit_continuation_requested_or_log(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: uuid::Uuid,
    reason: &str,
    actor: Option<MessageOrigin>,
    log_tag: &str,
) {
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationRequested {
                reason: reason.to_string(),
            },
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                actor,
                ..EventMeta::NONE
            },
        },
        log_tag,
    )
    .await;
}
