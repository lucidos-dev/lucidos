use super::super::*;
use super::*;

/// A trigger firing represents prior activity, so the row must land as
/// `Active`. Relying on the column default to reach `Active` means a future
/// default change silently re-introduces phantom drafts.
#[tokio::test]
async fn trigger_started_creates_active_thread_not_composing() {
    use crate::engine::thread_state::ThreadState;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-active".into(),
            trigger_name: Some("nightly".into()),
            prompt: Some("Run nightly".into()),
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
            go_to_review: false,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let state: String =
        sqlx::query_scalar("SELECT state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ThreadState::Active.as_str());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `Discarded` is terminal — a stale or replayed `TriggerStarted` against a
/// discarded thread must not resurrect it. The sibling MessageReceived and
/// SessionStarted ON CONFLICT branches gate on the same invariant.
#[tokio::test]
async fn trigger_started_does_not_resurrect_discarded_thread() {
    use crate::engine::thread_state::ThreadState;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadDiscarded { actor: None },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TriggerStarted {
            trigger_id: "t-stale".into(),
            trigger_name: Some("nightly".into()),
            prompt: Some("Run nightly".into()),
            invocation: Some(crate::engine::thread_events::TriggerInvocation::Schedule),
            origin: None,
            go_to_review: false,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let state: String =
        sqlx::query_scalar("SELECT state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ThreadState::Discarded.as_str());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Discarding a draft must also flip `archive_state` from the default
/// `'inbox'` to `'archived'` — otherwise the discarded row lingers in any
/// inbox-scoped query (drawer REVIEW badge, post-archive sibling picker)
/// even though the row has no events the user can interact with.
#[tokio::test]
async fn thread_discarded_sets_archive_state_to_archived() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "claude_code".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let archive_state_before: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(archive_state_before, "inbox");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadDiscarded { actor: None },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let archive_state_after: String =
        sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(archive_state_after, "archived");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A row in `thread_summaries` represents a thread with prior activity; only
/// `ThreadStarted` creates a draft and sets state explicitly. The default
/// must therefore be `Active` so any insert path that forgets to specify
/// state can't conjure a phantom draft.
#[tokio::test]
async fn thread_summaries_state_column_defaults_to_active() {
    use crate::engine::thread_state::ThreadState;
    let (pool, db_name) = setup_test_db().await;
    let thread_id = Uuid::new_v4();

    sqlx::query("INSERT INTO thread_summaries (thread_id) VALUES ($1)")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();

    let state: String =
        sqlx::query_scalar("SELECT state FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ThreadState::Active.as_str());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A composing thread that auto-archives without ever being sent must still
/// surface its intended channel — otherwise the drawer renders it as
/// "Lucidos" even when the user toggled CC. Source mirrors compose_mode for
/// composing threads; later send events overwrite if they disagree.
#[tokio::test]
async fn thread_started_with_claude_code_mode_sets_source_to_claude_code() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "claude_code".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (source, compose_mode): (String, Option<String>) =
        sqlx::query_as("SELECT source, compose_mode FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(compose_mode.as_deref(), Some("claude_code"));
    assert_eq!(source, "claude_code");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Lucidos mode maps to `source = 'chat'` so the drawer pill matches the
/// table-default behaviour and the existing CASE branches in MessageReceived
/// (which key off `source = 'chat'` to detect "no prior assertion").
#[tokio::test]
async fn thread_started_with_lucidos_mode_sets_source_to_chat() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let source: String =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(source, "chat");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the "Failed to send message: 409" toast on a draft whose
/// stale `source` no longer matches the actual send. A composing thread that
/// toggled mode to CC and back to Lucidos lands in `thread_summaries` with
/// `source='claude_code'` (PUT compose CASE writes it). When the user clicks
/// Send the chat handler now (correctly) skips the continuity check for
/// composing threads — but the projection still has to overwrite the lagged
/// source, otherwise the thread renders as Claude Code in the drawer pill
/// despite having been sent via Lucidos.
#[tokio::test]
async fn message_received_overrides_stale_compose_source_on_composing_to_active() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // Draft toggled CC at some point — source now claims claude_code.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "claude_code".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let pre_source: String =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        pre_source, "claude_code",
        "draft source seeded as claude_code"
    );

    // User toggled back to Lucidos and sent — MessageReceived arrives on the
    // chat channel. The projection must overwrite the stale source.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "send via lucidos".into(),
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

    let (state, source): (String, String) =
        sqlx::query_as("SELECT state, source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "active");
    assert_eq!(
        source, "chat",
        "composing → active must adopt the actual send's channel, got {source}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: `register_thread_queued`'s 60s eviction used to leak a
/// `ResponseCanceled` (no actor) onto the evicted thread because the
/// downstream stop arm read `is_shutdown=false` and defaulted to
/// "user-driven cancel" semantics. The frontend then rendered "Canceled"
/// next to the affected exchange — misleading users into thinking they
/// pressed Stop. The fix emits `ResponseAborted` with `actor=System`
/// directly from the eviction path; this test pins the actor and event
/// type the frontend needs to render "Aborted" instead.
#[tokio::test]
async fn stuck_thread_eviction_emits_aborted_with_system_actor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let request_id = Uuid::new_v4();

    // Anchor the request: a chat MessageReceived stamps a row in events
    // that latest_originating_event_id can find.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "first message".into(),
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
            event_id: Some(request_id),
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Empty agent_sessions — chat thread, no Claude Code session involved.
    let agent_sessions = tokio::sync::Mutex::new(std::collections::HashMap::new());

    // Empty active_threads: no live handle, so the anchor comes from the
    // `latest_originating_event_id` fallback this thread's events seed.
    let active_threads =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    crate::engine::emit_stuck_thread_eviction_abort(
        &bus,
        &pool,
        &agent_sessions,
        &active_threads,
        thread_id,
    )
    .await;

    let (event_type, actor_kind, req_id, status): (String, Option<String>, Option<String>, String) =
        sqlx::query_as(
            "SELECT e.event_type, e.payload->'actor'->>'kind', \
                e.payload->>'request_event_id', ts.status \
             FROM events e JOIN thread_summaries ts ON ts.thread_id = e.aggregate_id::uuid \
             WHERE e.aggregate_id = $1 AND e.event_type IN \
                ('ResponseAborted','ResponseCanceled','ResponseFailed','ResponseGenerated') \
             ORDER BY e.sequence DESC LIMIT 1",
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("a terminal event must be persisted by the eviction");

    assert_eq!(event_type, "ResponseAborted");
    assert_eq!(actor_kind.as_deref(), Some("system"));
    assert_eq!(req_id.as_deref(), Some(request_id.to_string().as_str()));
    // 'failed' (not 'idle') so the UI shows the error indicator — eviction is
    // a hard failure, not a clean dismissal.
    assert_eq!(status, "failed");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: a chat thread woken from `ChildThreadCompleted` (parent reacting
/// to a child's finish) carries the CTC's id as `request_event_id` on every
/// event in that turn. The eviction/restart abort must stamp the same CTC id,
/// not the most recent `MessageReceived` from a previous turn. Without this,
/// the abort routes via `reqIdRedirect` into the WRONG (old, already-completed)
/// exchange — `groupIntoExchanges` never pushes the abort into the active
/// `UserQuestionAsked` exchange, so its status stays `awaiting-answer` and the
/// question card looks pristine and clickable even though the response was
/// killed by an engine restart.
#[tokio::test]
async fn stuck_thread_eviction_uses_child_thread_completed_as_req_id_for_chat() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let old_mr_id = Uuid::new_v4();
    let ctc_id = Uuid::new_v4();
    let child_thread_id = Uuid::new_v4();

    // Older MessageReceived — the previous turn that completed cleanly.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "previous question".into(),
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
            event_id: Some(old_mr_id),
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Close out the old turn so the in-flight "thing being killed" is the CTC
    // wake, not the MR. Without this the test would be ambiguous about which
    // originating event the eviction should pick.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "previous answer".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            request_event_id: Some(old_mr_id),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // ChildThreadCompleted — the parent woke from a finished child. This is
    // the originating event of the now-in-flight turn that the eviction is
    // about to abort. Every event the chat agent emits in this turn carries
    // `request_event_id = ctc_id`.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChildThreadCompleted {
            child_thread_id,
            child_thread_title: Some("child".into()),
            status: crate::engine::thread_events::ChildCompletionStatus::Success,
            summary: "child finished".into(),
            pending_change_ids: vec![],
        },
        meta: EventMeta {
            event_id: Some(ctc_id),
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    // Empty agent_sessions — chat thread, not CC.
    let agent_sessions = tokio::sync::Mutex::new(std::collections::HashMap::new());

    // Empty active_threads: no live handle, so the anchor comes from the
    // `latest_originating_event_id` fallback this thread's events seed.
    let active_threads =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    crate::engine::emit_stuck_thread_eviction_abort(
        &bus,
        &pool,
        &agent_sessions,
        &active_threads,
        thread_id,
    )
    .await;

    let req_id: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'request_event_id' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ResponseAborted' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("ResponseAborted must be persisted by the eviction");

    assert_eq!(
        req_id.as_deref(),
        Some(ctc_id.to_string().as_str()),
        "chat eviction must stamp the most recent originating event (CTC), \
         not the older MessageReceived — otherwise the abort routes to the \
         wrong exchange in the frontend and the pending question card stays clickable",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Sibling structural assertion for the CC branch — CC parents woken from a
/// finished child carry `ChildThreadCompleted` as their turn's originating
/// event (the `AgentUserInput { kind: WakeFromChild, origin_event_id: ctc_id }`
/// flows into `run_session`'s `EventMeta::request_event_id`). The eviction
/// CC branch and the post-restart recovery use `CC_ORIGINATING_EVENT_TYPES`
/// — verifying the constant covers CTC keeps the CC-side mirror of the
/// chat regression above structurally impossible. Without this and the chat
/// pin together, an unrelated rename or refactor could drop CTC from one
/// list and the bug regresses silently for that channel.
#[test]
fn originating_event_type_lists_include_child_thread_completed() {
    use crate::engine::agent_session::{CC_ORIGINATING_EVENT_TYPES, CHAT_ORIGINATING_EVENT_TYPES};
    assert!(
        CHAT_ORIGINATING_EVENT_TYPES.contains(&"ChildThreadCompleted"),
        "chat list missing CTC — restart/eviction/Continue would route aborts \
         to the wrong exchange for child-wake turns"
    );
    assert!(
        CC_ORIGINATING_EVENT_TYPES.contains(&"ChildThreadCompleted"),
        "CC list missing CTC — same bug as the chat case for CC parents woken \
         from a finished child via notify_parent_of_child_completion"
    );
    assert!(
        CC_ORIGINATING_EVENT_TYPES.contains(&"CodingAgentUserMessageSent"),
        "CC list missing CCUMS — CC follow-up turns within an existing session \
         would not resolve to the right originating event"
    );
}

/// I3: a child's `ResponseFailed` with a giant error string (panic + stack
/// trace, hostile user input, etc.) must NOT propagate the full text into
/// the parent's `ChildThreadCompleted.summary`. The failure path caps at
/// 200 chars + "… (truncated)" — separate from the success path's 2000-char
/// budget, because failures are usually noise we don't want dominating the
/// parent's resume context.
#[tokio::test]
async fn child_thread_completed_failure_summary_capped_at_200_chars() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    // Parent + chat child wired up so the fan-in path runs.
    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            text: "do thing".into(),
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
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            text: "subtask".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Agent,
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

    // 5000-char error — well over both the 200 (failure) and 2000 (success)
    // caps. Should be truncated to ≤ ~213 chars (200 + ellipsis suffix).
    let huge_error = "X".repeat(5000);
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseFailed { error: huge_error },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let row: (serde_json::Value,) = sqlx::query_as(
        "SELECT payload FROM events \
         WHERE aggregate_id = $1 AND event_type = 'ChildThreadCompleted' \
         ORDER BY created LIMIT 1",
    )
    .bind(parent_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("ChildThreadCompleted must be persisted on parent");
    let summary = row
        .0
        .get("summary")
        .and_then(|v| v.as_str())
        .expect("summary field present")
        .to_string();
    let status = row
        .0
        .get("status")
        .and_then(|v| v.as_str())
        .expect("status field present");

    assert_eq!(
        status, "failure",
        "status must be failure for ResponseFailed"
    );
    // 200 chars of payload + "… (truncated)" suffix.
    assert!(
        summary.ends_with("… (truncated)"),
        "long failure summary must carry the truncation marker, got len={}, summary={:?}",
        summary.len(),
        summary
    );
    // Total length: 200 chars of "X" + the ellipsis suffix. The ellipsis is
    // 13 chars but as bytes it's 15 (the "…" char takes 3 bytes). Assert
    // the byte length is small enough that no stack-trace flooding is
    // possible — << 250 bytes.
    assert!(
        summary.len() <= 250,
        "failure summary must be truncated at 200 chars (+ short suffix), got len={} bytes",
        summary.len()
    );
    // And the prefix must be the original payload (not garbage).
    assert!(
        summary.starts_with("XXX"),
        "summary must keep the prefix of the original error, got: {:?}",
        summary
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: a system-actor synthetic `ToolResult` (the recovery sweep's
/// backfill that pairs an orphan `ToolCalled` so the next LLM call doesn't
/// trip the Anthropic API's "tool_use without tool_result" rule) must NOT
/// resurrect a terminated thread to `status='running'`. The activity-event
/// arm's normal "bump to Running" exists as defense-in-depth against
/// premature `CodingAgentIdled` drift on a *live* turn — system-stamped
/// activity events arrive long after the turn is already settled (see
/// `recover_orphan_tool_calls` in `engine/chat/recovery.rs`), so the bump is
/// wrong for them. Without this guard, every engine restart that finds
/// orphan ToolCalled events on already-terminated threads silently parks
/// those threads in the Active section forever.
#[tokio::test]
async fn system_actor_activity_event_does_not_resurrect_terminated_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();

    // 1. Live exchange: user message arrives, thread goes Running.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "do a thing".into(),
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

    // 2. Engine crashes mid-tool: ToolCalled writes, ToolResult never does.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolCalled {
            name: "run_python".into(),
            args: serde_json::json!({"code": "print(1)"}),
            description: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // 3. Engine restarts: the chat orphan-thread sweep emits ResponseAborted
    //    (EngineShutdown) with a SYSTEM actor. Nobody promised to resume this
    //    one, so for a chat thread (coding_agent_proposed=false) it maps to
    //    status='failed'.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: "interrupted".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: EventMeta::with_actor(Some(MessageOrigin::system())),
    })
    .await
    .unwrap();

    let after_abort: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after_abort, "failed",
        "a system-actor EngineShutdown abort is not the switch fingerprint, so a \
         thread with coding_agent_proposed=false settles at failed"
    );

    // 4. Tool-orphan sweep emits the synthetic ToolResult (system actor) to
    //    pair the orphan ToolCalled. This MUST NOT resurrect status to
    //    'running' — the thread's last lifecycle event is still the abort
    //    above; nothing has actually re-entered the LLM loop.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolResult {
            name: "run_python".into(),
            result: "[Tool execution interrupted by engine restart, original ToolCalled event_id: \
                     00000000-0000-0000-0000-000000000000]"
                .into(),
            images: vec![],
            success: false,
            tool_called_event_id: None,
        },
        meta: EventMeta::with_actor(Some(MessageOrigin::system())),
    })
    .await
    .unwrap();

    let after_synthetic: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after_synthetic, "failed",
        "system-actor synthetic ToolResult must not resurrect status to 'running'. \
         The recovery backfill is not live activity."
    );

    // 5. Sanity check: a live (no-actor) ToolResult arriving while the
    //    thread is still Running keeps the existing "bump to running"
    //    behavior. This guards against the fix accidentally killing the
    //    defense-in-depth path the activity arm exists for.
    let live_thread = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: live_thread,
        event: ThreadEvent::MessageReceived {
            text: "live".into(),
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
        thread_id: live_thread,
        event: ThreadEvent::ToolResult {
            name: "search".into(),
            result: "ok".into(),
            images: vec![],
            success: true,
            tool_called_event_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    let live_status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(live_thread)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        live_status, "running",
        "live (no-actor) ToolResult must still bump status to 'running' — \
         the System-actor guard must not regress the live-activity path"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: an interrupted **coding-agent** thread must keep the red
/// `failed` status dot for as long as an interrupted **Lucidos Agent** (chat)
/// thread does. Before this guard only chat threads kept it.
///
/// The asymmetry was not in the abort itself — both channels emit the same
/// `ResponseAborted` — but in what lands *after* it. A restart emits the
/// boundary abort while the coding-agent subprocess is still alive, so its
/// final buffered output arrives once the terminal event is already persisted:
/// `external_terminal_emitted` suppresses the duplicate TERMINAL, not the
/// activity stream, so `run_session` keeps forwarding
/// `CodingAgentTextStreamed` / `CodingAgentToolResult` from the drain
/// (observed in production ~13 ms after the abort). The activity arm's
/// unconditional "bump back to running" then erased the verdict, and the
/// `CodingAgentIdled` closing the turn — whose CASE only preserves a status
/// that *still* reads 'failed' — settled the row to 'idle'. A chat thread's
/// loop emits nothing after its own terminator, so it never lost the dot.
///
/// Runs both backends: the drain lives in the backend-agnostic
/// `agent_session` layer, so Codex produces the identical shape.
#[tokio::test]
async fn interrupted_coding_agent_thread_keeps_paused_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let status_of = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    for coding_agent in [
        crate::runtime::CodingAgent::ClaudeCode,
        crate::runtime::CodingAgent::Codex,
    ] {
        let agent = coding_agent.as_str();
        let thread_id = Uuid::new_v4();
        let cc_meta = || EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        };

        // 1. Live coding-agent turn.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                text: "fix the thing".into(),
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
            meta: cc_meta(),
        })
        .await
        .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                coding_agent,
                session_id: format!("{agent}-session"),
                branch: String::new(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();

        // 2. User hits *Switch to new version*: the teardown emits the boundary
        //    abort while the subprocess is still draining. The DEVICE actor is
        //    half the switch fingerprint (`AbortCause::promises_auto_resume`),
        //    and it is what makes this a paused turn rather than a failed one:
        //    the same pair is what the resume gates key on, so a system-actor
        //    abort here would be a turn nothing is coming back for.
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
                channel: Some(EventChannel::ClaudeCode),
                actor: Some(MessageOrigin::Device {
                    device_id: "dev-1".into(),
                    label: "My MacBook".into(),
                }),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "[{agent}] a device-attributed EngineShutdown abort is the user's own \
             switch, so it must settle the interrupted thread at paused, never at \
             the red failed"
        );

        // 3. The dying subprocess's trailing output. Live activity (no actor),
        //    so the System-actor recovery guard does not cover it.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentTextStreamed {
                text: "\n\nCommitting, then running the suite.".into(),
                coding_agent,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentToolResult {
                name: "Bash".into(),
                result: "ok".into(),
                coding_agent,
                tool_use_id: String::new(),
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "[{agent}] trailing drain output must not resurrect the aborted turn \
             to 'running'. The abort is the turn's verdict."
        );

        // 4. The idle that closes the interrupted turn.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: None,
                coding_agent,
                reason: Some("engine_restart_interrupt".into()),
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "[{agent}] CodingAgentIdled must not downgrade the interrupted \
             turn's paused verdict to 'idle'"
        );

        // 5. …nor may the session teardown that follows it.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionEnded {
                reason: SessionEndReason::Shutdown,
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "[{agent}] SessionEnded must not downgrade the interrupted turn's \
             paused verdict to 'idle'"
        );

        // 6. The verdict is sticky, not wedged: resuming clears it.
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationRequested {
                reason: "user_clicked_continue".into(),
            },
            meta: cc_meta(),
        })
        .await
        .unwrap();
        assert_eq!(
            status_of(thread_id).await,
            "running",
            "[{agent}] a real start event must still clear the error status"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `paused` is the promise "the engine is bringing this turn back", so only the
/// abort that carries such a promise may write it. Three shapes go through the
/// real projection here, differing ONLY in the pair the verdict reads:
///
/// 1. `EngineShutdown` + device actor: the user's own *Switch to new version*,
///    which both resume gates auto-resume. Paused.
/// 2. `EngineShutdown` + system actor: a teardown nobody requested (`stop.sh`,
///    an external SIGUSR1, ctrl-c). No gate picks it up. Note this is now the
///    ONLY way to get this pair: since 2026-08-07 every emit in one teardown
///    reads the same `LucidosEngine::teardown_actor`, so a user switch cannot
///    produce it for a thread that merely started late.
/// 3. `RecoveryAfterRestart` + system actor: the crash boundary, and the shape
///    `settle_unresumed_switch_threads` emits to WITHDRAW a promise this boot
///    could not keep. Withdrawing it behind a reassuring pause glyph, and out of
///    the needs-attention count, is the bug this test pins shut.
///
/// The last two must read `failed`: each keeps its Continue button, and `failed`
/// is what puts the thread where the user will look.
#[tokio::test]
async fn only_a_user_switch_teardown_settles_a_thread_at_paused() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let device = MessageOrigin::Device {
        device_id: "dev-1".into(),
        label: "My MacBook".into(),
    };

    for (case, cause, actor, expected) in [
        (
            "user switch teardown",
            crate::engine::thread_events::AbortCause::EngineShutdown,
            device.clone(),
            "paused",
        ),
        (
            "system-actor shutdown fallback",
            crate::engine::thread_events::AbortCause::EngineShutdown,
            MessageOrigin::system(),
            "failed",
        ),
        (
            "boot recovery / withdrawn resume promise",
            crate::engine::thread_events::AbortCause::RecoveryAfterRestart,
            MessageOrigin::system(),
            "failed",
        ),
    ] {
        let thread_id = Uuid::new_v4();
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                text: "do the thing".into(),
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
            thread_id,
            event: ThreadEvent::ResponseAborted {
                text: "interrupted".into(),
                images: vec![],
                model: None,
                reasoning_effort: None,
                cause,
            },
            meta: EventMeta::with_actor(Some(actor)),
        })
        .await
        .unwrap();

        let status: String =
            sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            status, expected,
            "{case}: {cause:?} must settle the thread at {expected}"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression: the shutdown sweep's phantom `ResponseCanceled` must not walk an
/// interrupted **Lucidos Agent** thread's error status back to 'idle'.
///
/// `shutdown_active_threads` emits `ResponseAborted`, then cancels the loop,
/// whose own `ResponseCanceled` follows. The sweep's docstring called the
/// phantom harmless because "ResponseAborted takes precedence in status
/// derivation", which is only true of the exchange label;
/// `thread_summaries.status` is last-write-wins, so the cancel used to erase the
/// verdict. `preserving_verdict` is what stops it.
///
/// The sweep anchors both emits on the in-flight turn now, so
/// `emit_response_canceled`'s idempotency gate usually pairs them and no phantom
/// is produced at all. This test deliberately reproduces the unpaired ordering
/// anyway: the gate needs an anchor to match on, and a turn that never got far
/// enough to record one still reaches this shape.
#[tokio::test]
async fn shutdown_phantom_cancel_does_not_clear_the_abort_error_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let status_of = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "summarize the log".into(),
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

    // The sweep's abort — note the absent `request_event_id`, which is exactly
    // why the gate below can't suppress the loop's cancel.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: "This response was interrupted by an engine shutdown.".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: EventMeta::with_actor(Some(MessageOrigin::system())),
    })
    .await
    .unwrap();
    // System actor, so this is NOT the switch fingerprint: no resume gate will
    // pick this thread up, and 'failed' is what keeps it in the attention count
    // with its Continue button.
    assert_eq!(status_of(thread_id).await, "failed");

    // `cancel_all_threads` wakes the agentic loop's cancel arm moments later.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        status_of(thread_id).await,
        "failed",
        "the loop's phantom ResponseCanceled must not clear the interrupted \
         thread's status. The abort is the turn's verdict."
    );

    // A cancel of a genuinely new turn still settles the thread: the start
    // event clears the verdict first, so Stop behaves exactly as before.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "try again".into(),
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
    assert_eq!(status_of(thread_id).await, "running");
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        status_of(thread_id).await,
        "idle",
        "a real user Stop on a fresh turn must still settle the thread to idle"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Sending a message clears ALL compose fields — including `compose_selection`.
/// The MessageReceived / SessionStarted / ThreadDiscarded projection arms wipe
/// the per-thread compose draft so a stale draft can't linger; `compose_selection`
/// (the per-draft dropdown picks, added alongside text/images/mode) must be wiped
/// in lockstep, or a sent thread retains a ghost selection in the DB that a reload
/// would rehydrate. Regression guard for the frontend "peer-sent follow-up
/// preserved as a draft" fix's backend half.
#[tokio::test]
async fn message_received_clears_compose_selection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    // Composing thread with a stored per-draft selection (as a compose PUT would
    // leave it).
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE thread_summaries \
         SET compose_text = 'half-typed', compose_selection = '{\"scope\":{\"kind\":\"lucidos\"}}'::jsonb \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "the actual message".into(),
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
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (compose_text, compose_selection): (String, Option<serde_json::Value>) = sqlx::query_as(
        "SELECT compose_text, compose_selection FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(compose_text, "", "compose_text must be cleared on send");
    assert_eq!(
        compose_selection, None,
        "compose_selection must be cleared on send, in lockstep with the other compose fields"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The discard arm wipes `compose_selection` too — a discarded draft leaves no
/// ghost dropdown picks behind for a replay/reload to resurrect.
#[tokio::test]
async fn thread_discarded_clears_compose_selection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE thread_summaries \
         SET compose_text = 'half-typed', compose_selection = '{\"model\":\"claude-opus-4-8\"}'::jsonb \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadDiscarded { actor: None },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let compose_selection: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT compose_selection FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        compose_selection, None,
        "compose_selection must be cleared on discard, in lockstep with the other compose fields"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Seed an active thread parked on a question, with `draft` sitting in its
/// compose fields — the shape a device leaves behind when the user typed into
/// the composer while an `ask_user_question` was on screen.
async fn seed_thread_awaiting_answer(bus: &EventBus, pool: &PgPool, draft: &str) -> Uuid {
    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE thread_summaries \
         SET state = 'active', compose_text = $2, \
             compose_selection = '{\"scope\":{\"kind\":\"lucidos\"}}'::jsonb \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .bind(draft)
    .execute(pool)
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu-1".into(),
            cc_session_id: "sess-1".into(),
            question: "Proceed?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    thread_id
}

/// Read the compose fields the answer arm is allowed to touch.
async fn read_compose(pool: &PgPool, thread_id: Uuid) -> (String, Option<serde_json::Value>) {
    sqlx::query_as(
        "SELECT compose_text, compose_selection FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Typed text answering a question IS a submitted draft, but it never becomes a
/// `MessageReceived` (chat/process/run.rs reroutes it), so the send arm's clear
/// never runs for it. Without this the draft stays in `thread_summaries` and
/// re-syncs to every device whenever the submitting client's compose PUT doesn't
/// land — a resurrected ghost draft.
#[tokio::test]
async fn free_text_answer_clears_the_draft_it_submitted() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id =
        seed_thread_awaiting_answer(&bus, &pool, "night has passed by, any progress?\n").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu-1".into(),
            // The client trims before submitting; the stored draft may not be.
            answer: AnswerKind::FreeText {
                text: "night has passed by, any progress?".into(),
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (compose_text, compose_selection) = read_compose(&pool, thread_id).await;
    assert_eq!(
        compose_text, "",
        "a free-text answer must clear the draft it submitted, trimming aside"
    );
    assert_eq!(
        compose_selection, None,
        "compose_selection must clear in lockstep with the text it belonged to"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The clear must also be ANNOUNCED. This path emits no `MessageReceived`, and
/// the submitting client's own compose PUT (which normally broadcasts the empty
/// state) may never land — so without this side effect, peers holding the same
/// draft keep showing it until their next thread-summary snapshot. A thread
/// event won't do: delivery can lag arbitrarily behind the transaction, so the
/// frontend only accepts a compose REPORT as evidence of the server's current
/// state (`serverDraft` in `store/actions/compose.ts`).
#[tokio::test]
async fn free_text_answer_broadcasts_the_cleared_compose_state() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id =
        seed_thread_awaiting_answer(&bus, &pool, "night has passed by, any progress?").await;
    let mut rx = bus.subscribe();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu-1".into(),
            answer: AnswerKind::FreeText {
                text: "night has passed by, any progress?".into(),
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let mut broadcast: Option<(String, Vec<String>, Option<String>)> = None;
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::System(SystemEvent::ThreadComposeChanged {
            id,
            text,
            image_hashes,
            origin_device_id,
            ..
        }) = &emitted.typed
        {
            if *id == thread_id {
                broadcast = Some((text.clone(), image_hashes.clone(), origin_device_id.clone()));
            }
        }
    }
    let (text, image_hashes, origin_device_id) =
        broadcast.expect("clearing the answered draft must broadcast ThreadComposeChanged");
    assert_eq!(text, "", "the broadcast reports the cleared state");
    assert!(image_hashes.is_empty(), "images clear with the text");
    assert_eq!(
        origin_device_id, None,
        "the server's own report belongs to no device — nobody may suppress it as their echo"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The ordinary send path owes the same announcement. `sendCompose` cancels its
/// compose PUT and relies on this projection's clear, so without the broadcast a
/// peer mirroring the draft keeps showing it until its next summary reload.
#[tokio::test]
async fn sending_a_draft_broadcasts_the_cleared_compose_state() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    sqlx::query("UPDATE thread_summaries SET compose_text = 'the draft' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut rx = bus.subscribe();

    emit_thread_message(&bus, thread_id, None, "the draft").await;

    let mut announced = false;
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::System(SystemEvent::ThreadComposeChanged { id, text, .. }) = &emitted.typed
        {
            if *id == thread_id {
                assert_eq!(text, "", "the broadcast reports the cleared state");
                announced = true;
            }
        }
    }
    assert!(
        announced,
        "sending a thread's draft must announce that the server no longer holds it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A message that CREATES the thread announces nothing. There is no compose
/// slot to consume, the epoch stays at its storage default, and no device has
/// heard of the thread yet, so an SSE frame would reach nobody.
#[tokio::test]
async fn a_message_that_creates_the_thread_broadcasts_no_compose_change() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let mut rx = bus.subscribe();

    emit_thread_message(&bus, thread_id, None, "straight to send").await;

    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::System(SystemEvent::ThreadComposeChanged { id, .. }) = &emitted.typed {
            assert_ne!(*id, thread_id, "the thread was created by this message");
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A send on an existing thread announces even when the engine held NO draft,
/// because the *compose epoch* moved. This is the reported bug's exact shape:
/// the client's draft PUT was still in flight, so the engine had nothing to
/// clear, and the write landed after the message and rewrote the draft the send
/// had just consumed. The device needs the new epoch to fence its next write,
/// and the announcement is how it gets it without a round trip.
#[tokio::test]
async fn sending_without_a_stored_draft_still_announces_the_new_epoch() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    let mut rx = bus.subscribe();

    emit_thread_message(&bus, thread_id, None, "typed and sent inside the debounce").await;

    let mut announced_epoch = None;
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::System(SystemEvent::ThreadComposeChanged {
            id, compose_epoch, ..
        }) = &emitted.typed
        {
            if *id == thread_id {
                announced_epoch = Some(*compose_epoch);
            }
        }
    }
    assert_eq!(
        announced_epoch,
        Some(1),
        "the submission consumed the compose slot, so the epoch advanced and peers must hear it"
    );

    let stored: (i64,) =
        sqlx::query_as("SELECT compose_epoch FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored.0, 1,
        "the stored epoch is what later writes fence on"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An answer whose text has NOT reached storage yet still advances the epoch.
///
/// This is the answer-path form of the reported bug. The answering device's
/// compose PUT is still in flight, so `compose_text` holds the older value and
/// the content match finds nothing to clear. Without a bump, that stalled write
/// lands after the answer and resurrects the submitted text as a live draft. The
/// unrelated text a peer may be writing must survive either way, so the CLEAR
/// stays conditional while the fence does not.
#[tokio::test]
async fn an_answer_advances_the_epoch_even_when_it_cleared_no_stored_draft() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id =
        seed_thread_awaiting_answer(&bus, &pool, "a different half-typed thought").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu-1".into(),
            answer: AnswerKind::FreeText {
                text: "the answer, typed and sent inside the debounce".into(),
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let row: (String, i64) = sqlx::query_as(
        "SELECT compose_text, compose_epoch FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row.0, "a different half-typed thought",
        "an answer that was not the stored draft must leave that draft alone"
    );
    assert_eq!(
        row.1, 1,
        "but it must still fence the write that carried the answer text"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A `ContinuationStarted` is not a submission. The user clicked Continue, they
/// did not send, so their draft survives AND the epoch must stay put: bumping
/// it would refuse the next perfectly good write for nothing.
#[tokio::test]
async fn a_chat_continue_leaves_the_compose_epoch_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadStarted {
            mode: "lucidos".into(),
            actor: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE thread_summaries SET compose_text = 'half a follow-up' WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: Some("user_clicked_continue".into()),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let row: (String, i64) = sqlx::query_as(
        "SELECT compose_text, compose_epoch FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "half a follow-up", "a Continue consumes no draft");
    assert_eq!(row.1, 0, "and therefore moves no epoch");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The mirror of the above: no clear, no announcement. An answer that matched
/// no stored draft must not tell peers their compose is empty.
#[tokio::test]
async fn answer_that_clears_nothing_broadcasts_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id =
        seed_thread_awaiting_answer(&bus, &pool, "a different half-typed thought").await;
    let mut rx = bus.subscribe();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu-1".into(),
            answer: AnswerKind::FreeText {
                text: "yes, go ahead".into(),
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::System(SystemEvent::ThreadComposeChanged { id, .. }) = &emitted.typed {
            assert_ne!(
                *id, thread_id,
                "an answer that cleared no draft must not announce an empty compose"
            );
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The clear is scoped to the draft that was actually submitted. A peer holding
/// a DIFFERENT in-progress draft must keep it — unlike a send (which replaces
/// the composer's contents by definition), answering a question says nothing
/// about text the user is still writing elsewhere.
#[tokio::test]
async fn answer_leaves_a_different_stored_draft_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id =
        seed_thread_awaiting_answer(&bus, &pool, "a different half-typed thought").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu-1".into(),
            answer: AnswerKind::FreeText {
                text: "yes, go ahead".into(),
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (compose_text, _) = read_compose(&pool, thread_id).await;
    assert_eq!(
        compose_text, "a different half-typed thought",
        "an answer that submitted other text must not wipe an unrelated draft"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An answer carries no image hashes — the composer refuses to submit one with
/// attachments — so a stored draft that HAS images is not the thing that was
/// submitted, even when its text matches. Wiping it would destroy a peer's
/// attachments; the client's supersede rule matches on text AND hashes for the
/// same reason.
#[tokio::test]
async fn answer_leaves_an_image_bearing_draft_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = seed_thread_awaiting_answer(&bus, &pool, "look at this").await;
    sqlx::query(
        "UPDATE thread_summaries SET compose_images = '[\"hash-a\"]'::jsonb WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .unwrap();

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu-1".into(),
            answer: AnswerKind::FreeText {
                text: "look at this".into(),
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (compose_text, _) = read_compose(&pool, thread_id).await;
    let compose_images: serde_json::Value =
        sqlx::query_scalar("SELECT compose_images FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        compose_text, "look at this",
        "a text-only answer must not consume a draft carrying attachments"
    );
    assert_eq!(
        compose_images,
        serde_json::json!(["hash-a"]),
        "the peer's attachments must survive"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A click-only answer submits no composer text at all, so it clears nothing —
/// the draft is still the user's unsent work.
#[tokio::test]
async fn option_only_answer_clears_no_draft() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = seed_thread_awaiting_answer(&bus, &pool, "still writing this").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu-1".into(),
            answer: AnswerKind::MultiSelected {
                option_ids: vec!["opt-a".into()],
                text: None,
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (compose_text, _) = read_compose(&pool, thread_id).await;
    assert_eq!(
        compose_text, "still writing this",
        "an options-only answer submits no composer text and must clear nothing"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A multi-select answer folds the prompt textarea's text in alongside the
/// toggled options — that text came from the composer, so the matching draft
/// clears just like the free-text case.
#[tokio::test]
async fn multi_select_answer_clears_the_draft_it_folded_in() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = seed_thread_awaiting_answer(&bus, &pool, "and also check the logs").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: "tu-1".into(),
            answer: AnswerKind::MultiSelected {
                option_ids: vec!["opt-a".into()],
                text: Some("and also check the logs".into()),
            },
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (compose_text, _) = read_compose(&pool, thread_id).await;
    assert_eq!(
        compose_text, "",
        "typed text folded into a multi-select answer is a submitted draft"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
