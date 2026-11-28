use super::super::event_bus::{BusEvent, EventBus};
use super::super::thread_events::{ActorMode, EventMeta, ThreadEvent};
use super::super::{InjectedPrompt, InjectedPromptKind};
use super::{
    append_injected_prompts_to_messages, emit_iteration_cap_response_generated,
    emit_prompt_injected_event, ensure_terminator_emitted, filter_removed_queued_prompts,
    round_backstop_message, tool_call_cap_message,
};
use crate::engine::failure_terminator::ensure_failure_terminator_emitted;
use crate::llm::{Message, MessageContent};
use crate::test_support::{message_received, setup_test_db, teardown_test_db};
use uuid::Uuid;

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

#[tokio::test]
async fn iteration_cap_emits_response_generated_terminator() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, origin_id, "long task").await;

    let default_cap = crate::core::prefs::MAX_TOOL_CALLS.default_number() as usize;
    let configured_cap = 42;
    assert_ne!(configured_cap, default_cap);
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
        !msg.contains(&default_cap.to_string()),
        "the message must not name the default when a different cap fired: {}",
        msg
    );
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

#[tokio::test]
async fn prompt_injected_does_not_collide_with_optimistic_id() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let client_event_id = Uuid::new_v4();
    let base_meta = anchor_request(&bus, thread_id, client_event_id, "follow-up").await;

    let prompt = InjectedPrompt {
        text: "actually do it differently".to_string(),
        event_id: Some(client_event_id),
        mode: ActorMode::Human,
        spawning_event_id: None,
        images: None,
        origin: None,
        kind: crate::engine::InjectedPromptKind::UserText,
    };

    emit_prompt_injected_event(&bus, thread_id, &base_meta, &prompt).await;

    let prompt_injected_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM events \
         WHERE aggregate_id = $1 AND event_type = 'PromptInjected'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("PromptInjected must persist (no duplicate-key error)");

    assert_ne!(
        prompt_injected_id, client_event_id,
        "PromptInjected must get a fresh id, not the optimistic MessageReceived id"
    );

    let request_link: Option<String> =
        sqlx::query_scalar("SELECT payload->>'request_event_id' FROM events WHERE id = $1")
            .bind(prompt_injected_id)
            .fetch_one(&pool)
            .await
            .expect("query");
    assert_eq!(
        request_link.as_deref(),
        Some(client_event_id.to_string().as_str()),
        "request_event_id must still link PromptInjected back to the originating request"
    );

    let injected_link: Option<String> =
        sqlx::query_scalar("SELECT payload->>'injected_message_id' FROM events WHERE id = $1")
            .bind(prompt_injected_id)
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

    let workspace = tempfile::tempdir().unwrap();
    let mut messages = Vec::<Message>::new();
    let appended = append_injected_prompts_to_messages(
        workspace.path(),
        &bus,
        thread_id,
        &meta,
        &mut messages,
        prompts,
    )
    .await;
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
         WHERE aggregate_id = $1 AND event_type = 'PromptInjected' \
         ORDER BY created",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .expect("PromptInjected rows must persist");
    assert_eq!(
        injected_links,
        vec![
            first_followup_id.to_string(),
            second_followup_id.to_string()
        ]
    );

    teardown_test_db(&db_name).await;
}

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
        images: Some(vec![crate::engine::types::ChatImage {
            base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==".to_string(),
            mime_type: "image/png".to_string(),
        }]),
        origin: None,
        kind: crate::engine::InjectedPromptKind::UserText,
    }];

    let workspace = tempfile::tempdir().unwrap();
    let mut messages = Vec::<Message>::new();
    let appended = append_injected_prompts_to_messages(
        workspace.path(),
        &bus,
        thread_id,
        &meta,
        &mut messages,
        prompts,
    )
    .await;

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

#[tokio::test]
async fn ensure_terminator_emitted_scopes_check_to_request_event_id() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

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

#[tokio::test]
async fn an_in_loop_failure_is_not_terminated_twice() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, origin_id, "do work").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseFailed {
            error: "OpenAI streaming error [502]: Provider returned error".into(),
            spend_cap: None,
            agent_exit: None,
            provider_error: None,
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
            spend_cap: None,
            agent_exit: None,
            provider_error: None,
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

#[tokio::test]
async fn emit_response_canceled_is_idempotent_when_terminator_exists_for_request() {
    use crate::engine::thread_events::{emit_response_canceled, AbortCause, CancelCause};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let origin_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, origin_id, "fix the bug").await;

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
            restore_to_compose: false,
            compose: None,
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

async fn mock_engine(db: &str, workspace: &std::path::Path) -> crate::engine::LucidosEngine {
    crate::engine::LucidosEngine::new(
        workspace.to_path_buf(),
        &crate::test_support::test_db_url(db),
        crate::llm::ProviderBuildContext {
            default_model: "mock".into(),
            model_is_mock: true,
            vertex_project_id: String::new(),
            vertex_location: crate::llm::vertex::location_handle(String::new()),
            vertex_token_cache: None,
            model_registry: Default::default(),
            not_served: Default::default(),
            claude_gateway_access: None,
            chatgpt_plan_access: None,
        },
        std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
        Box::new(crate::engine::provider_adapters::ProviderAdapters::default()),
    )
    .await
    .unwrap()
}

fn device(id: &str) -> crate::engine::thread_events::MessageOrigin {
    crate::engine::thread_events::MessageOrigin::Device {
        device_id: id.to_string(),
    }
}

#[tokio::test]
async fn the_cancel_actor_is_drained_and_never_overwrites_an_author() {
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = mock_engine(&db, workspace.path()).await;
    let thread_id = Uuid::new_v4();
    let _registration = engine
        .try_register_thread(thread_id)
        .expect("a fresh thread registers");

    engine.cancel_thread(thread_id, Some(device("phone")));
    let stamped = super::meta_with_cancel_actor(&engine, thread_id, &EventMeta::NONE);
    assert_eq!(stamped.actor, Some(device("phone")));
    let drained = super::meta_with_cancel_actor(&engine, thread_id, &EventMeta::NONE);
    assert_eq!(
        drained.actor, None,
        "a stale device must not reach the next cancel"
    );

    engine.cancel_thread(thread_id, Some(device("phone")));
    let author = EventMeta {
        actor: Some(device("laptop")),
        ..EventMeta::NONE
    };
    let kept = super::meta_with_cancel_actor(&engine, thread_id, &author);
    assert_eq!(
        kept.actor,
        Some(device("laptop")),
        "the cancel-clicker must not replace the message author"
    );

    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_cancel_cause_names_a_redirect_once_and_a_stop_otherwise() {
    use crate::engine::thread_events::CancelCause;

    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = mock_engine(&db, workspace.path()).await;
    let thread_id = Uuid::new_v4();
    let _registration = engine
        .try_register_thread(thread_id)
        .expect("a fresh thread registers");

    engine.cancel_thread(thread_id, None);
    assert_eq!(
        super::cancel_cause_for_turn(&engine, thread_id),
        CancelCause::UserStop
    );

    engine.cancel_thread_for_followup(thread_id, None);
    assert_eq!(
        super::cancel_cause_for_turn(&engine, thread_id),
        CancelCause::SupersededByFollowup
    );
    assert_eq!(
        super::cancel_cause_for_turn(&engine, thread_id),
        CancelCause::UserStop,
        "a stale redirect flag must not relabel the next turn"
    );

    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_tool_called_row_never_carries_a_hardcoded_postgres_password() {
    use super::helpers::HelpersExt;

    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = mock_engine(&db, workspace.path()).await;
    let thread_id = Uuid::new_v4();
    let call = crate::llm::provider::ToolCall {
        id: "call-1".to_string(),
        name: crate::llm::tool_names::RUN_BASH.to_string(),
        arguments: serde_json::json!({
            "command": "psql postgres://lucidos:hunter2@localhost:5432/lucidos -c 'select 1'"
        }),
        thought_signature: None,
    };

    let event_id = engine
        .emit_tool_called(thread_id, &EventMeta::NONE, &call, 1, 5)
        .await
        .expect("the ToolCalled row persists");

    let (description, args): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT payload->>'description', payload->>'args' FROM events WHERE id = $1",
    )
    .bind(event_id)
    .fetch_one(&pool)
    .await
    .expect("query");
    let (description, args) = (description.unwrap_or_default(), args.unwrap());
    assert!(!args.contains("hunter2"), "args leaked: {args}");
    assert!(args.contains("lucidos:***@localhost"), "args: {args}");
    assert!(
        !description.contains("hunter2"),
        "description leaked: {description}"
    );

    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_tool_result_row_names_the_call_it_answers() {
    use super::helpers::HelpersExt;

    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = mock_engine(&db, workspace.path()).await;
    let thread_id = Uuid::new_v4();
    let called = Uuid::new_v4();
    let mut split = super::split_tool_result("plain result");

    engine
        .emit_tool_result(
            thread_id,
            &EventMeta::NONE,
            "read_file",
            &mut split,
            true,
            Some(called),
        )
        .await;

    let stamped: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'tool_called_event_id' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ToolResult'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("the ToolResult row persists");
    assert_eq!(stamped, Some(called.to_string()));

    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_empty_image_list_is_neither_image_shaped_nor_pinned() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let meta = anchor_request(&bus, thread_id, Uuid::new_v4(), "original request").await;
    let prompts = vec![InjectedPrompt {
        text: "no picture after all".to_string(),
        event_id: Some(Uuid::new_v4()),
        mode: ActorMode::Human,
        spawning_event_id: None,
        images: Some(vec![]),
        origin: None,
        kind: crate::engine::InjectedPromptKind::UserText,
    }];

    let workspace = tempfile::tempdir().unwrap();
    let mut messages = Vec::<Message>::new();
    let appended = append_injected_prompts_to_messages(
        workspace.path(),
        &bus,
        thread_id,
        &meta,
        &mut messages,
        prompts,
    )
    .await;

    assert!(appended.appended);
    assert!(appended.image_message_idxs.is_empty());
    assert!(matches!(messages[0].content, MessageContent::Text(_)));

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_keep_in_a_cancelled_round_is_returned_but_never_recorded() {
    use super::super::chat::process::working_understanding as wu;
    use super::read_working_understanding;

    const KEPT: &str = "evt-0123456789abcdef0123456789abcdef";
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let mut document = wu::WorkingUnderstanding::default();
    let mut todo = vec![];
    let (_, applied) = read_working_understanding(
        &bus,
        thread_id,
        &format!("[WORKING UNDERSTANDING]\n[KEEP OPEN]\n{KEPT}\n[/WORKING UNDERSTANDING]"),
        wu::ReplyEnd::Truncated,
        &mut document,
        &mut todo,
        &None,
    )
    .await;

    assert_eq!(applied.keep_open, vec![KEPT.to_string()]);
    let recorded: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ContextKeptOpen'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("query");
    assert_eq!(recorded, 0);

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_re_emitted_checklist_carries_the_notes_through() {
    use super::super::chat::process::working_understanding as wu;
    use super::read_working_understanding;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let mut document = wu::WorkingUnderstanding::default();
    let mut todo = vec![];
    let notes = Some("wrote before the mode".to_string());
    read_working_understanding(
        &bus,
        thread_id,
        "[WORKING UNDERSTANDING]\nbody\n[TODO]\n- [ ] fix it\n[/WORKING UNDERSTANDING]",
        wu::ReplyEnd::Complete,
        &mut document,
        &mut todo,
        &notes,
    )
    .await;

    let stored: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'notes' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'TodoListWritten'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("the checklist is re-emitted");
    assert_eq!(stored, notes);

    teardown_test_db(&db_name).await;
}

#[test]
fn the_system_prompt_quotes_the_prefix_both_engine_limit_messages_carry() {
    let prompt = include_str!("chat/process/system_prompt.rs");
    for msg in [tool_call_cap_message(7), round_backstop_message(7)] {
        let prefix = msg.split_whitespace().next().unwrap();
        assert!(
            prompt.contains(&format!("\"{prefix}\" prefix")),
            "the system prompt must quote {prefix}"
        );
    }
}
