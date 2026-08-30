//! Regression tests for the agentic loop's terminator-emission and
//! injection-event-id contracts. Both bugs surfaced together when a
//! long-running thread silently went zombie after hitting the
//! per-turn iteration cap (no terminator → UI stuck on "running"),
//! and every follow-up the user typed collided with the original
//! `MessageReceived` row on `events_pkey` because the optimistic
//! client UUID was reused as the persisted `UserPromptInjected` id.

use super::super::event_bus::{BusEvent, EventBus};
use super::super::thread_events::{ActorMode, EventMeta, ThreadEvent};
use super::super::{InjectedPrompt, InjectedPromptKind};
use super::{
    append_injected_prompts_to_messages, emit_iteration_cap_response_generated,
    emit_user_prompt_injected_event, ensure_failure_terminator_emitted, ensure_terminator_emitted,
    filter_removed_queued_prompts, round_backstop_message, tool_call_cap_message,
};
use crate::core::DEFAULT_MAX_TOOL_CALLS;
use crate::llm::{Message, MessageContent};
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

fn message_received(text: &str) -> ThreadEvent {
    ThreadEvent::MessageReceived {
        voice_session_id: None,
        text: text.to_string(),
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
    }
}

/// Persist a MessageReceived row with the given client-provided event_id and
/// return the canonical EventMeta used by all events that belong to that
/// request (matches what `run_agentic_loop` builds from `origin_id`).
async fn anchor_request(
    bus: &EventBus,
    thread_id: Uuid,
    client_event_id: Uuid,
    text: &str,
) -> EventMeta {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: message_received(text),
        meta: EventMeta {
            event_id: Some(client_event_id),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("MessageReceived must persist");
    EventMeta {
        request_event_id: Some(client_event_id),
        ..EventMeta::NONE
    }
}

// ---------------------------------------------------------------------------
// Iteration-cap terminator
// ---------------------------------------------------------------------------

/// Regression: hitting the per-turn iteration cap used to `return Ok(...)`
/// silently — no `ResponseGenerated`/`ResponseFailed`/`ResponseAborted` ever
/// hit the events table, so the frontend treated the thread as still
/// running. This test asserts the cap path emits a `ResponseGenerated`
/// terminator carrying the engine-limit sentinel string.
#[tokio::test]
async fn iteration_cap_emits_response_generated_terminator() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, origin_id, "long task").await;

    // A non-default cap on purpose: the message must name the cap that actually
    // fired, not the compiled-in default. Passing 500 here would let a
    // regression that reverted to the constant pass unnoticed.
    let configured_cap = 42;
    assert_ne!(configured_cap, DEFAULT_MAX_TOOL_CALLS);
    let msg = emit_iteration_cap_response_generated(
        &bus,
        thread_id,
        &meta,
        vec![],
        None,
        None,
        tool_call_cap_message(configured_cap),
    )
    .await;

    assert!(
        msg.contains("[ENGINE-LIMIT]"),
        "engine-limit sentinel must be present in the returned text: {}",
        msg
    );
    assert!(
        msg.contains(&configured_cap.to_string()),
        "the user-facing message must name the cap that fired, got: {}",
        msg
    );
    assert!(
        !msg.contains(&DEFAULT_MAX_TOOL_CALLS.to_string()),
        "the message must not name the default when a different cap fired: {}",
        msg
    );
    // Hitting the cap is the one moment the user has a reason to raise it, and a
    // packaged user has no constant to edit. The pointer is the whole mechanism
    // by which they learn the setting exists, so pin both halves: the clickable
    // panel link and the row's name.
    assert!(
        msg.contains("[Settings](settings)"),
        "the message must carry a clickable Settings link, got: {}",
        msg
    );
    assert!(
        msg.contains("Max tool calls"),
        "the message must name the setting row, got: {}",
        msg
    );

    let (count, request_link): (i64, Option<String>) = sqlx::query_as(
        "SELECT COUNT(*), MAX(payload->>'request_event_id') \
         FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseGenerated'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("query");

    assert_eq!(
        count, 1,
        "iteration cap must emit exactly one ResponseGenerated"
    );
    assert_eq!(
        request_link.as_deref(),
        Some(origin_id.to_string().as_str()),
        "the terminator must carry request_event_id linking back to the originating MessageReceived"
    );

    teardown_test_db(&db_name).await;
}

/// The two backstops must not borrow each other's wording. The round backstop
/// fires when the loop span without a tool call, so claiming the user hit their
/// tool-call limit would be false, and it would carry the one prefix the system
/// prompt tells the model to trust. Raising the setting would not help there
/// either, so only the real cap message points at Settings.
#[test]
fn the_two_engine_limit_messages_say_different_things() {
    let cap = tool_call_cap_message(42);
    let backstop = round_backstop_message(600);

    for msg in [&cap, &backstop] {
        assert!(
            msg.starts_with("[ENGINE-LIMIT]"),
            "both terminators carry the sentinel the prompt tells the model to trust: {msg}"
        );
        assert!(
            msg.contains("Send any message to continue"),
            "both must tell the user the turn is resumable: {msg}"
        );
    }

    assert!(cap.contains("42") && cap.contains("[Settings](settings)"));
    assert!(
        !backstop.contains("[Settings](settings)"),
        "raising the cap does not help a turn that never reached it: {backstop}"
    );
    assert!(
        !backstop.contains("limit of"),
        "the backstop must not read as the user's cap being reached: {backstop}"
    );
    assert!(
        backstop.contains("600"),
        "the backstop should say how far the turn got: {backstop}"
    );
}

// ---------------------------------------------------------------------------
// UserPromptInjected event_id collision
// ---------------------------------------------------------------------------

/// Regression: the inject path used to write `inject_meta.event_id =
/// prompt.event_id`, reusing the client-provided UUID that
/// `chat::process` had already persisted as `MessageReceived.id`. The
/// emit silently failed under `emit_or_log`, the loop kept going, the UI
/// never got the SSE — and worse, the event was lost forever. This test
/// drives the inject helper directly and asserts the persisted UPI row
/// gets a fresh id (different from the prompt's optimistic id) and the
/// emit succeeds.
#[tokio::test]
async fn user_prompt_injected_does_not_collide_with_optimistic_id() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let client_event_id = Uuid::new_v4();
    let base_meta = anchor_request(&bus, thread_id, client_event_id, "follow-up").await;

    let prompt = InjectedPrompt {
        text: "actually do it differently".to_string(),
        // Identical to the MessageReceived row's id — exactly the production
        // shape that triggered the duplicate-key crash.
        event_id: Some(client_event_id),
        mode: ActorMode::Human,
        spawning_event_id: None,
        images: None,
        origin: None,
        kind: crate::engine::InjectedPromptKind::UserText,
    };

    emit_user_prompt_injected_event(&bus, thread_id, &base_meta, &prompt).await;

    let upi_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM events \
         WHERE aggregate_id = $1 AND event_type = 'UserPromptInjected'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("UserPromptInjected must persist (no duplicate-key error)");

    assert_ne!(
        upi_id, client_event_id,
        "UserPromptInjected must get a fresh id, not the optimistic MessageReceived id"
    );

    let request_link: Option<String> =
        sqlx::query_scalar("SELECT payload->>'request_event_id' FROM events WHERE id = $1")
            .bind(upi_id)
            .fetch_one(&pool)
            .await
            .expect("query");
    assert_eq!(
        request_link.as_deref(),
        Some(client_event_id.to_string().as_str()),
        "request_event_id must still link UPI back to the originating request"
    );

    // injected_message_id carries the MessageReceived id forward so the
    // renderer can collapse the duplicate "Auto-prompt sent" panel into the
    // existing user message.
    let injected_link: Option<String> =
        sqlx::query_scalar("SELECT payload->>'injected_message_id' FROM events WHERE id = $1")
            .bind(upi_id)
            .fetch_one(&pool)
            .await
            .expect("query");
    assert_eq!(
        injected_link.as_deref(),
        Some(client_event_id.to_string().as_str()),
        "injected_message_id must point at the paired MessageReceived",
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn append_injected_prompts_coalesces_messages_but_emits_each_audit_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, origin_id, "original request").await;
    let first_followup_id = Uuid::new_v4();
    let second_followup_id = Uuid::new_v4();
    let prompts = vec![
        InjectedPrompt {
            text: "first queued update".to_string(),
            event_id: Some(first_followup_id),
            mode: ActorMode::Human,
            spawning_event_id: None,
            images: None,
            origin: None,
            kind: crate::engine::InjectedPromptKind::UserText,
        },
        InjectedPrompt {
            text: "second queued update".to_string(),
            event_id: Some(second_followup_id),
            mode: ActorMode::Human,
            spawning_event_id: None,
            images: None,
            origin: None,
            kind: crate::engine::InjectedPromptKind::UserText,
        },
    ];

    let mut messages = Vec::<Message>::new();
    let appended =
        append_injected_prompts_to_messages(&bus, thread_id, &meta, &mut messages, prompts).await;
    assert!(
        appended.appended,
        "coalescing should append one LLM message"
    );
    assert!(
        appended.image_message_idxs.is_empty(),
        "text-only injections must not be pinned against image trimming"
    );
    assert_eq!(messages.len(), 1);
    let MessageContent::Blocks(blocks) = &messages[0].content else {
        panic!("multiple injected prompts must become one block message");
    };
    assert_eq!(blocks.len(), 2);

    let injected_links: Vec<String> = sqlx::query_scalar(
        "SELECT payload->>'injected_message_id' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'UserPromptInjected' \
         ORDER BY created",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .expect("UserPromptInjected rows must persist");
    assert_eq!(
        injected_links,
        vec![
            first_followup_id.to_string(),
            second_followup_id.to_string()
        ]
    );

    teardown_test_db(&db_name).await;
}

/// Regression for the reported bug's first half: the user attached a screenshot
/// to a message that arrived while the thread was already working, so it was
/// queued and later injected mid-turn. The loop advanced `user_message_idx` on
/// injection but nothing recorded that the appended message carried image bytes
/// — trim pass 0 keeps images on the last message plus the pinned ones only, so
/// the model's very next tool call stripped it and the agent went blind to a
/// picture the user had just sent.
///
/// The helper must report the appended message's index so the loop can pin it.
#[tokio::test]
async fn append_injected_prompts_reports_image_bearing_message_for_pinning() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, origin_id, "original request").await;

    let prompts = vec![InjectedPrompt {
        text: "feil i appen".to_string(),
        event_id: Some(Uuid::new_v4()),
        mode: ActorMode::Human,
        spawning_event_id: None,
        images: Some(vec![crate::api::ChatImage {
            // 1x1 transparent PNG.
            base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==".to_string(),
            mime_type: "image/png".to_string(),
        }]),
        origin: None,
        kind: crate::engine::InjectedPromptKind::UserText,
    }];

    let mut messages = Vec::<Message>::new();
    let appended =
        append_injected_prompts_to_messages(&bus, thread_id, &meta, &mut messages, prompts).await;

    assert!(appended.appended);
    assert_eq!(
        appended.image_message_idxs,
        vec![messages.len() - 1],
        "an injected message carrying images must be reported so the loop pins it \
         against trim pass 0"
    );
    let MessageContent::Blocks(blocks) = &messages[0].content else {
        panic!("an image-bearing injection must become a block message");
    };
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, crate::llm::ContentBlock::Image { .. })),
        "the injected image bytes must actually reach the message"
    );

    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// Defensive post-loop guard
// ---------------------------------------------------------------------------

/// If a future bug causes the loop to return without emitting a terminator
/// (the iteration-cap path was the most recent example, but any new branch
/// could regress), the post-loop guard must catch it and emit
/// `ResponseAborted` so the UI doesn't get stuck. This test sets up a
/// request with activity but no terminator, calls the guard, and asserts
/// `ResponseAborted` lands.
#[tokio::test]
async fn ensure_terminator_emitted_recovers_silent_exit() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, origin_id, "do work").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolResult {
            name: "edit_file".into(),
            result: "ok".into(),
            images: vec![],
            success: true,
            tool_called_event_id: None,
        },
        meta: meta.clone(),
    })
    .await
    .expect("activity emit ok");

    ensure_terminator_emitted(&bus, &pool, thread_id, origin_id, None).await;

    let (count, request_link): (i64, Option<String>) = sqlx::query_as(
        "SELECT COUNT(*), MAX(payload->>'request_event_id') \
         FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("query");

    assert_eq!(
        count, 1,
        "guard must emit a ResponseAborted when the loop returned without one"
    );
    assert_eq!(
        request_link.as_deref(),
        Some(origin_id.to_string().as_str()),
        "the defensive abort must carry request_event_id back to the originating MessageReceived"
    );

    teardown_test_db(&db_name).await;
}

/// Inverse: if a terminator already exists for the request, the guard must
/// NOT double-emit. A spurious second terminator would create phantom
/// "Aborted" exchanges in the UI on every successful loop completion.
#[tokio::test]
async fn ensure_terminator_emitted_is_idempotent_when_terminator_exists() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, origin_id, "do work").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "all done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: meta.clone(),
    })
    .await
    .expect("ResponseGenerated emit ok");

    ensure_terminator_emitted(&bus, &pool, thread_id, origin_id, None).await;

    let aborted_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("query");
    assert_eq!(
        aborted_count, 0,
        "guard must be a no-op when a terminator was already emitted"
    );

    teardown_test_db(&db_name).await;
}

/// The guard's terminator-detection key is `request_event_id`, not just
/// "any terminator on the thread". A previous exchange's
/// `ResponseGenerated` must not satisfy the check for a later exchange
/// that died silently — otherwise the guard would never fire on
/// long-running threads (which is exactly the long-lived-thread shape).
#[tokio::test]
async fn ensure_terminator_emitted_scopes_check_to_request_event_id() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // Earlier exchange — completed cleanly.
    let earlier_origin = Uuid::new_v4();
    let earlier_meta = anchor_request(&bus, thread_id, earlier_origin, "first turn").await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "first answer".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: earlier_meta,
    })
    .await
    .expect("earlier terminator");

    // Current exchange — silently exited.
    let current_origin = Uuid::new_v4();
    let current_meta = anchor_request(&bus, thread_id, current_origin, "second turn").await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolResult {
            name: "edit_file".into(),
            result: "ok".into(),
            images: vec![],
            success: true,
            tool_called_event_id: None,
        },
        meta: current_meta.clone(),
    })
    .await
    .expect("activity emit ok");

    ensure_terminator_emitted(&bus, &pool, thread_id, current_origin, None).await;

    let aborted_link: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'request_event_id' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id.to_string())
    .fetch_optional(&pool)
    .await
    .expect("query");
    assert_eq!(
        aborted_link.as_deref(),
        Some(current_origin.to_string().as_str()),
        "guard must emit a ResponseAborted for the current request, ignoring earlier completed exchanges"
    );

    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// Failure terminator: one per exchange
// ---------------------------------------------------------------------------

/// Count every `ResponseFailed` row on a thread.
async fn failed_count(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseFailed'",
    )
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .expect("query")
}

/// Count the `ResponseFailed` rows anchored to one exchange. Counting by
/// anchor is the whole point: an unanchored row belongs to no exchange, and
/// that is what the gate could not see.
async fn failed_count_for(pool: &sqlx::PgPool, thread_id: Uuid, anchor: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseFailed' \
           AND payload->>'request_event_id' = $2",
    )
    .bind(thread_id.to_string())
    .bind(anchor.to_string())
    .fetch_one(pool)
    .await
    .expect("query")
}

/// A turn that dies BEFORE the agentic loop leaves the exchange open, and
/// the UI stuck on "running". The engine settles it, anchored, so the
/// callers no longer need a terminator of their own.
#[tokio::test]
async fn a_pre_loop_failure_is_settled_with_an_anchored_terminator() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    anchor_request(&bus, thread_id, origin_id, "do work").await;

    ensure_failure_terminator_emitted(&bus, &pool, thread_id, origin_id, None, "setup blew up")
        .await;

    assert_eq!(
        failed_count(&pool, thread_id).await,
        1,
        "a pre-loop failure must settle the exchange"
    );
    assert_eq!(
        failed_count_for(&pool, thread_id, origin_id).await,
        1,
        "the terminator must carry the anchor, or the gate cannot dedup it"
    );

    let error: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'error' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseFailed'",
    )
    .bind(thread_id.to_string())
    .fetch_optional(&pool)
    .await
    .expect("query");
    assert_eq!(error.as_deref(), Some("setup blew up"));

    teardown_test_db(&db_name).await;
}

/// The reported bug. A stream error inside the loop emits an ANCHORED
/// `ResponseFailed`, and the caller used to stack an unanchored copy on
/// top milliseconds later. Two rows meant every subscribed trigger fired
/// twice. One exchange gets one terminator.
#[tokio::test]
async fn an_in_loop_failure_is_not_terminated_twice() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, origin_id, "do work").await;

    // What the agentic loop emits when the provider stream errors.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseFailed {
            error: "OpenAI streaming error [502]: Provider returned error".into(),
        },
        meta,
    })
    .await
    .expect("loop terminator emit ok");

    ensure_failure_terminator_emitted(
        &bus,
        &pool,
        thread_id,
        origin_id,
        None,
        "OpenAI streaming error [502]: Provider returned error",
    )
    .await;

    assert_eq!(
        failed_count(&pool, thread_id).await,
        1,
        "the loop already settled this exchange"
    );

    teardown_test_db(&db_name).await;
}

/// The gate keys on `request_event_id`, so an earlier exchange's failure
/// must not swallow this one. Otherwise a thread that failed once would
/// never settle again, and every later turn would hang on "running".
#[tokio::test]
async fn an_earlier_failure_does_not_settle_the_current_exchange() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    let earlier_origin = Uuid::new_v4();
    let earlier_meta = anchor_request(&bus, thread_id, earlier_origin, "first turn").await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseFailed {
            error: "first failure".into(),
        },
        meta: earlier_meta,
    })
    .await
    .expect("earlier terminator");

    let current_origin = Uuid::new_v4();
    anchor_request(&bus, thread_id, current_origin, "second turn").await;
    ensure_failure_terminator_emitted(
        &bus,
        &pool,
        thread_id,
        current_origin,
        None,
        "second failure",
    )
    .await;

    assert_eq!(
        failed_count(&pool, thread_id).await,
        2,
        "the current exchange needs its own terminator"
    );
    assert_eq!(
        failed_count_for(&pool, thread_id, current_origin).await,
        1,
        "exactly one terminator must belong to the current exchange"
    );

    teardown_test_db(&db_name).await;
}

/// Regression: `/api/v1/restart` pre-emits `ResponseAborted{actor: device}` for
/// the in-flight chat thread, then `force_evict_chat_thread` cancels its
/// token. The agentic loop's cancel branches fire moments later. Without
/// the dedup gate inside `emit_response_canceled`, the loop's emit lands
/// on top of the pre-emitted abort and the timeline shows both
/// "Paused by restart" AND "Response canceled" boundaries stacked together.
/// The gate must read the request_event_id off `meta`, find the prior
/// terminator, and skip its own emit.
#[tokio::test]
async fn emit_response_canceled_is_idempotent_when_terminator_exists_for_request() {
    use crate::engine::thread_events::{emit_response_canceled, AbortCause, CancelCause};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, origin_id, "fix the bug").await;

    // Pre-emit the boundary abort (mirroring `/api/v1/restart`).
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
        meta: meta.clone(),
    })
    .await
    .expect("pre-emit ResponseAborted");

    // Loop's cancel branch fires after the token is cancelled.
    emit_response_canceled(
        &bus,
        &pool,
        thread_id,
        CancelCause::UserStop,
        String::new(),
        vec![],
        None,
        None,
        meta,
        "[Test] cancel branch",
    )
    .await;

    let canceled_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseCanceled'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("query");
    assert_eq!(
        canceled_count, 0,
        "emit_response_canceled must skip when a terminator already exists for the request"
    );

    teardown_test_db(&db_name).await;
}

/// Counterpart: with no prior terminator, the cancel emit must land. Guards
/// against a future refactor that hard-codes the skip and silently breaks
/// the user-clicks-Stop flow.
#[tokio::test]
async fn emit_response_canceled_lands_when_no_prior_terminator() {
    use crate::engine::thread_events::{emit_response_canceled, CancelCause};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, origin_id, "fix the bug").await;

    emit_response_canceled(
        &bus,
        &pool,
        thread_id,
        CancelCause::UserStop,
        "partial reply".into(),
        vec![],
        None,
        None,
        meta,
        "[Test] user stop",
    )
    .await;

    let canceled_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseCanceled'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("query");
    assert_eq!(
        canceled_count, 1,
        "emit must land when no prior terminator exists"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn filter_removed_queued_prompts_binds_thread_aggregate_id_as_text() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let kept_id = Uuid::new_v4();
    let removed_id = Uuid::new_v4();

    anchor_request(&bus, thread_id, removed_id, "drop").await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::QueuedMessageRemoved {
            removed_message_id: removed_id,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("QueuedMessageRemoved must persist");

    let prompts = vec![
        InjectedPrompt {
            text: "keep".into(),
            event_id: Some(kept_id),
            mode: ActorMode::Human,
            spawning_event_id: None,
            images: None,
            origin: None,
            kind: InjectedPromptKind::UserText,
        },
        InjectedPrompt {
            text: "drop".into(),
            event_id: Some(removed_id),
            mode: ActorMode::Human,
            spawning_event_id: None,
            images: None,
            origin: None,
            kind: InjectedPromptKind::UserText,
        },
    ];

    let filtered = filter_removed_queued_prompts(&pool, thread_id, prompts).await;
    assert_eq!(
        filtered.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
        vec!["keep"],
        "removed queued prompts must be filtered before ingestion"
    );

    teardown_test_db(&db_name).await;
}

// ---------------------------------------------------------------------------
// The working understanding survives a cancelled round
// ---------------------------------------------------------------------------

/// A stopped round used to lose the document and flush the raw markup instead.
/// The cancel arm returned before the parse block that persists and splices.
/// Twenty rounds of notes went with one Stop, and the user read the machinery.
///
/// This drives the helper both terminal paths share.
#[tokio::test]
async fn a_cancelled_round_persists_the_document_it_wrote() {
    use super::super::chat::process::working_understanding as wu;
    use super::read_working_understanding;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let partial = "Reading the file now.\n\
                   [WORKING UNDERSTANDING]\nthe file is 400 lines\n[/WORKING UNDERSTANDING]\n\
                   Next I will";
    let mut document = wu::WorkingUnderstanding::default();
    let mut todo = vec![];
    let (parsed, _) = read_working_understanding(
        &bus,
        thread_id,
        partial,
        wu::ReplyEnd::Truncated,
        &mut document,
        &mut todo,
        &None,
    )
    .await;

    let stored: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'document' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'WorkingUnderstandingWritten'",
    )
    .bind(thread_id.to_string())
    .fetch_optional(&pool)
    .await
    .expect("query");
    assert_eq!(
        stored.as_deref(),
        Some("the file is 400 lines"),
        "a cancelled round still wrote what it wrote"
    );

    let flushed = wu::strip_faulted_markup(&wu::splice_spans_out(partial, &parsed.spans));
    assert!(
        !flushed.contains(wu::MARKER_PREFIX),
        "and the transcript gets prose, not markup: {flushed}"
    );
    assert_eq!(flushed, "Reading the file now.\n\nNext I will");

    teardown_test_db(&db_name).await;
}

/// The checklist rides along, so a cancel does not strand the prompt bar on a
/// list the model had already moved past.
#[tokio::test]
async fn a_cancelled_round_persists_the_checklist_it_wrote() {
    use super::super::chat::process::working_understanding as wu;
    use super::read_working_understanding;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let mut document = wu::WorkingUnderstanding::default();
    let mut todo = vec![];
    read_working_understanding(
        &bus,
        thread_id,
        "[WORKING UNDERSTANDING]\nbody\n[TODO]\n- [x] read the file\n- [ ] fix it\n\
         [/WORKING UNDERSTANDING]",
        wu::ReplyEnd::Truncated,
        &mut document,
        &mut todo,
        &None,
    )
    .await;

    assert_eq!(todo.len(), 2, "the loop's own snapshot moved with the emit");
    let written: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events \
         WHERE aggregate_id = $1 AND event_type = 'TodoListWritten'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("query");
    assert_eq!(written, 1);

    teardown_test_db(&db_name).await;
}

/// A Stop landing mid-block keeps the document that was already there.
///
/// The parse takes an unclosed block to the end of the text, so the fragment
/// would arrive as a whole-document REPLACE and overwrite everything. The Stop
/// is what cut it, not the model, so the block is dropped and the old notes
/// stand. The user still reads prose: the markup goes with it.
#[tokio::test]
async fn a_stop_inside_the_block_leaves_the_old_document_standing() {
    use super::super::chat::process::working_understanding as wu;
    use super::read_working_understanding;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let partial = "On it.\n[WORKING UNDERSTANDING]\nthe fi";
    let mut document = wu::WorkingUnderstanding {
        body: "twenty rounds of notes".into(),
        constraints: "never touch the migrations".into(),
    };
    let mut todo = vec![];
    let (parsed, _) = read_working_understanding(
        &bus,
        thread_id,
        partial,
        wu::ReplyEnd::Truncated,
        &mut document,
        &mut todo,
        &None,
    )
    .await;

    assert_eq!(document.body, "twenty rounds of notes");
    assert_eq!(document.constraints, "never touch the migrations");
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 \
         AND event_type = 'WorkingUnderstandingWritten'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("query");
    assert_eq!(rows, 0, "nothing was finished, so nothing is recorded");

    let flushed = wu::strip_faulted_markup(&wu::splice_spans_out(partial, &parsed.spans));
    assert_eq!(flushed, "On it.");

    teardown_test_db(&db_name).await;
}

/// The same reply, ended by the model rather than by a Stop, still lands. An
/// opening it never closed is its own mistake, and the fault says so.
#[tokio::test]
async fn a_model_that_forgot_the_closing_marker_still_writes_its_document() {
    use super::super::chat::process::working_understanding as wu;
    use super::read_working_understanding;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let mut document = wu::WorkingUnderstanding::default();
    let mut todo = vec![];
    let (parsed, _) = read_working_understanding(
        &bus,
        thread_id,
        "On it.\n[WORKING UNDERSTANDING]\nthe file is 400 lines",
        wu::ReplyEnd::Complete,
        &mut document,
        &mut todo,
        &None,
    )
    .await;

    assert_eq!(document.body, "the file is 400 lines");
    assert!(
        parsed.faults.iter().any(|f| f.contains(wu::CLOSE)),
        "and it is told what it left out: {:?}",
        parsed.faults
    );

    teardown_test_db(&db_name).await;
}

/// A round that wrote nothing emits nothing. Otherwise every cancel would
/// append an identical document row, and the mode's own history would be noise.
#[tokio::test]
async fn a_cancelled_round_that_wrote_nothing_emits_nothing() {
    use super::super::chat::process::working_understanding as wu;
    use super::read_working_understanding;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let mut document = wu::WorkingUnderstanding {
        body: "notes from earlier".into(),
        constraints: String::new(),
    };
    let mut todo = vec![];
    read_working_understanding(
        &bus,
        thread_id,
        "Just thinking out loud here.",
        wu::ReplyEnd::Truncated,
        &mut document,
        &mut todo,
        &None,
    )
    .await;

    assert_eq!(document.body, "notes from earlier");
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 \
         AND event_type IN ('WorkingUnderstandingWritten', 'TodoListWritten')",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("query");
    assert_eq!(rows, 0);

    teardown_test_db(&db_name).await;
}
