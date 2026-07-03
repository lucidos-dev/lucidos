use super::*;
use super::aq_test_helpers::*;
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;



#[test]
fn validate_answer_accepts_selected_and_freetext_and_canceled() {
    let opts = vec![opt("opt-0", "A"), opt("opt-1", "B")];
    // Single-select question accepts Selected/FreeText/Canceled.
    assert!(validate_answer(
        &AnswerKind::Selected {
            option_id: "opt-0".into()
        },
        &opts,
        false
    )
    .is_ok());
    assert!(validate_answer(&AnswerKind::FreeText { text: "x".into() }, &opts, false).is_ok());
    assert!(validate_answer(&AnswerKind::Canceled, &opts, false).is_ok());

    // Multi-select question accepts the same fall-throughs (single Selected
    // is allowed — equivalent to MultiSelected with one id).
    assert!(validate_answer(
        &AnswerKind::Selected {
            option_id: "opt-0".into()
        },
        &opts,
        true
    )
    .is_ok());
    assert!(validate_answer(&AnswerKind::Canceled, &opts, true).is_ok());
}

#[test]
fn validate_answer_accepts_multi_selected_with_known_ids() {
    let opts = vec![opt("opt-0", "A"), opt("opt-1", "B"), opt("opt-2", "C")];
    let answer = AnswerKind::MultiSelected {
        option_ids: vec!["opt-0".into(), "opt-2".into()],
        text: None,
    };
    assert!(validate_answer(&answer, &opts, true).is_ok());
}

#[test]
fn validate_answer_accepts_multi_selected_with_only_text() {
    // Prompt-row Submit folds typed text into MultiSelected even when
    // no toggles are active. Empty option_ids + non-empty text is valid.
    let opts = vec![opt("opt-0", "A")];
    let answer = AnswerKind::MultiSelected {
        option_ids: vec![],
        text: Some("just text".into()),
    };
    assert!(validate_answer(&answer, &opts, true).is_ok());
}

#[test]
fn validate_answer_accepts_multi_selected_with_ids_and_text() {
    let opts = vec![opt("opt-0", "A"), opt("opt-1", "B")];
    let answer = AnswerKind::MultiSelected {
        option_ids: vec!["opt-0".into()],
        text: Some("plus this".into()),
    };
    assert!(validate_answer(&answer, &opts, true).is_ok());
}

#[test]
fn validate_answer_rejects_empty_multi_selected() {
    let opts = vec![opt("opt-0", "A")];
    let answer = AnswerKind::MultiSelected {
        option_ids: vec![],
        text: None,
    };
    let err = validate_answer(&answer, &opts, true).expect_err("must reject empty");
    assert!(
        err.contains("at least one"),
        "error must mention requirement; got {err:?}"
    );
}

#[test]
fn validate_answer_rejects_multi_selected_with_only_empty_text() {
    // Empty string is still empty — must be rejected like no text at all.
    let opts = vec![opt("opt-0", "A")];
    let answer = AnswerKind::MultiSelected {
        option_ids: vec![],
        text: Some(String::new()),
    };
    assert!(validate_answer(&answer, &opts, true).is_err());
}

#[test]
fn validate_answer_rejects_unknown_multi_selected_id() {
    let opts = vec![opt("opt-0", "A")];
    let answer = AnswerKind::MultiSelected {
        option_ids: vec!["opt-0".into(), "opt-99".into()],
        text: None,
    };
    let err = validate_answer(&answer, &opts, true).expect_err("must reject unknown id");
    assert!(
        err.contains("opt-99"),
        "error must surface the unknown id; got {err:?}"
    );
}

#[test]
fn validate_answer_rejects_multi_selected_for_single_select_question() {
    let opts = vec![opt("opt-0", "A")];
    let answer = AnswerKind::MultiSelected {
        option_ids: vec!["opt-0".into()],
        text: None,
    };
    let err = validate_answer(&answer, &opts, false).expect_err("single-select rejects multi");
    assert!(
        err.contains("single-select"),
        "error must explain mismatch; got {err:?}"
    );
}





/// No `agent_sessions` entry means `notify()` cannot reach a hook; the
/// answer would silently strand without a `ContinuationRequested`.
#[tokio::test]
async fn ensure_resume_emits_continuation_requested_when_no_live_session() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    let emitted = ensure_resume_after_answer(
        &bus,
        &sessions,
        thread_id,
        &AnswerKind::FreeText { text: "Y".into() },
        None,
    )
    .await;
    assert!(
        emitted,
        "must emit ContinuationRequested when agent_sessions has no entry"
    );
    assert_eq!(count_continuation_requests(&pool, thread_id).await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `process_exited == true` means the hook went down with the
/// subprocess; `notify()` can't wake it, so we still need a Continue spawn.
#[tokio::test]
async fn ensure_resume_emits_continuation_requested_when_session_exited() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let mut map = HashMap::new();
    map.insert(thread_id, make_session(true));
    let sessions = Arc::new(tokio::sync::Mutex::new(map));

    let emitted = ensure_resume_after_answer(
        &bus,
        &sessions,
        thread_id,
        &AnswerKind::FreeText { text: "Y".into() },
        None,
    )
    .await;
    assert!(
        emitted,
        "must emit ContinuationRequested when session exists but its subprocess has exited"
    );
    assert_eq!(count_continuation_requests(&pool, thread_id).await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Live subprocess: `notify()` already woke the in-flight hook. A
/// `ContinuationRequested` would race that and could spawn a duplicate.
#[tokio::test]
async fn ensure_resume_skips_emit_when_session_is_alive() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let mut map = HashMap::new();
    map.insert(thread_id, make_session(false));
    let sessions = Arc::new(tokio::sync::Mutex::new(map));

    let emitted = ensure_resume_after_answer(
        &bus,
        &sessions,
        thread_id,
        &AnswerKind::FreeText { text: "Y".into() },
        None,
    )
    .await;
    assert!(
        !emitted,
        "must NOT emit ContinuationRequested when subprocess is alive"
    );
    assert_eq!(count_continuation_requests(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `archive_thread` calls `answer_pending_question(.., Canceled)` to
/// resolve the question card right before `stop_agent`.
/// A `ContinuationRequested` here would race the imminent SessionEnded and
/// spawn a fresh subprocess for a thread the user just archived.
#[tokio::test]
async fn ensure_resume_skips_emit_for_canceled_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    let emitted =
        ensure_resume_after_answer(&bus, &sessions, thread_id, &AnswerKind::Canceled, None).await;
    assert!(
        !emitted,
        "Canceled is the archive sentinel and must never spawn a Continue"
    );
    assert_eq!(count_continuation_requests(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Live subprocess answer: arm the run-loop resume signal so the turn CC
/// continues (via the PreToolUse hook, off the `msg_tx` path) re-arms emission
/// instead of dropping its output as post-terminal stragglers.
#[tokio::test]
async fn arm_question_resume_sets_flag_on_live_session() {
    let thread_id = Uuid::new_v4();
    let mut map = HashMap::new();
    map.insert(thread_id, make_session(false)); // process_exited=false → live
    let sessions = Arc::new(tokio::sync::Mutex::new(map));

    let armed = arm_question_resume_if_live(
        &sessions,
        thread_id,
        &AnswerKind::FreeText { text: "Y".into() },
    )
    .await;
    assert!(armed, "must report a live subprocess was armed");
    assert!(
        sessions.lock().await.get(&thread_id).unwrap().question_resume_pending,
        "live session must have question_resume_pending set so the run loop self-heals"
    );
}

/// Exited subprocess: nothing to re-arm in-place — the no-live path spawns a
/// fresh `--resume` turn (new run loop, clean flags), so leave the signal off.
#[tokio::test]
async fn arm_question_resume_skips_exited_session() {
    let thread_id = Uuid::new_v4();
    let mut map = HashMap::new();
    map.insert(thread_id, make_session(true)); // process_exited=true
    let sessions = Arc::new(tokio::sync::Mutex::new(map));

    let armed = arm_question_resume_if_live(
        &sessions,
        thread_id,
        &AnswerKind::FreeText { text: "Y".into() },
    )
    .await;
    assert!(!armed, "an exited subprocess must not be armed");
    assert!(
        !sessions.lock().await.get(&thread_id).unwrap().question_resume_pending,
        "exited session must leave question_resume_pending false"
    );
}

/// Absent session: nothing to arm.
#[tokio::test]
async fn arm_question_resume_skips_absent_session() {
    let thread_id = Uuid::new_v4();
    let sessions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    let armed = arm_question_resume_if_live(
        &sessions,
        thread_id,
        &AnswerKind::FreeText { text: "Y".into() },
    )
    .await;
    assert!(!armed, "an absent session must not be armed");
}

/// Canceled is the archive/teardown sentinel — never arm a resume, mirroring
/// `ensure_resume_after_answer` / `emit_resume_marker_for_cc_answer`.
#[tokio::test]
async fn arm_question_resume_skips_canceled_even_when_live() {
    let thread_id = Uuid::new_v4();
    let mut map = HashMap::new();
    map.insert(thread_id, make_session(false)); // live
    let sessions = Arc::new(tokio::sync::Mutex::new(map));

    let armed = arm_question_resume_if_live(&sessions, thread_id, &AnswerKind::Canceled).await;
    assert!(!armed, "Canceled must never arm a resume");
    assert!(
        !sessions.lock().await.get(&thread_id).unwrap().question_resume_pending,
        "Canceled must leave question_resume_pending false even on a live session"
    );
}


/// Cancel-stamp path (HTTP `claude_code_stop`, `archive_thread`) always
/// follows the Canceled answer with `stop_agent`, so the marker would
/// strand on the timeline as an empty `Thinking ✓` placeholder under
/// the QuestionCard's own ✓ Cancel disabled-red state. The frontend
/// guard (`isCanceledQuestionDivider` in `thread-events.ts`) hides the
/// CC panel for the same reason; this engine guard keeps the underlying
/// event store clean of the useless emit.
#[tokio::test]
async fn emit_resume_marker_skips_emit_for_canceled_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    let emitted =
        emit_resume_marker_for_cc_answer(
            &bus,
            thread_id,
            &AnswerKind::Canceled,
            None,
            crate::runtime::CodingAgent::ClaudeCode,
        )
        .await;
    assert!(
        !emitted,
        "Canceled answer must not emit a resume marker — no CC turn follows"
    );
    assert_eq!(count_coding_agent_prompt_sent(&pool, thread_id).await, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Active answers (Selected / FreeText / MultiSelected) MUST keep the
/// marker — CC is about to process the synthetic AskUserQuestion
/// tool_result and produce its next assistant message; without the
/// Thinking placeholder, the steps area sits empty during that round-trip.
#[tokio::test]
async fn emit_resume_marker_emits_for_selected_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    let emitted = emit_resume_marker_for_cc_answer(
        &bus,
        thread_id,
        &AnswerKind::Selected {
            option_id: "opt-0".into(),
        },
        None,
        crate::runtime::CodingAgent::ClaudeCode,
    )
    .await;
    assert!(emitted, "active answers must emit the Thinking placeholder");
    assert_eq!(count_coding_agent_prompt_sent(&pool, thread_id).await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn emit_resume_marker_emits_for_free_text_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;

    let emitted = emit_resume_marker_for_cc_answer(
        &bus,
        thread_id,
        &AnswerKind::FreeText {
            text: "purple".into(),
        },
        None,
        crate::runtime::CodingAgent::ClaudeCode,
    )
    .await;
    assert!(emitted, "FreeText answer must keep the marker");
    assert_eq!(count_coding_agent_prompt_sent(&pool, thread_id).await, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}


/// Baseline: an unanswered question with nothing after it is returned by
/// both lookup variants — the turn is still in flight.
#[tokio::test]
async fn both_lookups_return_question_when_unanswered_and_no_terminator() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu-pending").await;

    assert_eq!(
        lookup_pending_question_tool_use_id(&pool, thread_id)
            .await
            .as_deref(),
        Some("toolu-pending"),
        "broad lookup must return the live unanswered question"
    );
    assert_eq!(
        lookup_active_question_tool_use_id(&pool, thread_id)
            .await
            .as_deref(),
        Some("toolu-pending"),
        "active-only lookup must return the live unanswered question"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}


/// The chat-side `ask_user_question` tool emits `UserQuestionAsked` with
/// `meta.channel = Chat`. `answer_pending_question` must be able to read
/// that back so it knows to skip the CC-specific resume side-effects
/// (`CodingAgentPromptSent` marker, `ContinuationRequested` spawn). This helper
/// is the lookup primitive.
#[tokio::test]
async fn lookup_question_channel_returns_chat_for_chat_emitter() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    // Chat threads bootstrap on `MessageReceived`; `SessionStarted` is
    // CC-only per the lifecycle validator.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "ask me about colors".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: crate::engine::thread_events::ActorMode::Human,
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
    .expect("MessageReceived emit")
    .expect("MessageReceived persisted");
    let tool_use_id = "toolu-chat";
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.into(),
            cc_session_id: String::new(),
            question: "Pick".into(),
            options: vec![opt("opt-0", "A")],
            worktree_path: None,
            multi_select: false,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("UserQuestionAsked emit")
    .expect("UserQuestionAsked persisted");

    let channel = lookup_question_channel(&pool, thread_id, tool_use_id)
        .await
        .expect("lookup must succeed");
    assert_eq!(
        channel,
        Some(EventChannel::Chat),
        "chat-emitted question must be read back as channel=Chat"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// CC's hook continues to emit `channel = ClaudeCode`. The lookup must
/// surface that unchanged so the existing CC resume path still fires.
#[tokio::test]
async fn lookup_question_channel_returns_claude_code_for_cc_emitter() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let tool_use_id = "toolu-cc";
    emit_user_question(&bus, thread_id, tool_use_id).await;

    let channel = lookup_question_channel(&pool, thread_id, tool_use_id)
        .await
        .expect("lookup must succeed");
    assert_eq!(channel, Some(EventChannel::ClaudeCode));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Legacy rows persisted before the channel field was always written can
/// return `None`. `answer_pending_question` must treat `None` as
/// "default to today's CC behaviour" so the upgrade is backward-compat.
#[tokio::test]
async fn lookup_question_channel_returns_none_when_channel_absent() {
    let (pool, db_name) = setup_test_db().await;

    let thread_id = Uuid::new_v4();
    let tool_use_id = "toolu-legacy";
    // Insert a row whose payload predates the channel field. We bypass
    // the bus to avoid having to construct a `SessionStarted` lineage
    // for a synthetic legacy event.
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'UserQuestionAsked', $3, NOW(), $2)"
    )
    .bind(Uuid::new_v4())
    .bind(thread_id)
    .bind(serde_json::json!({
        "tool_use_id": tool_use_id,
        "cc_session_id": "sid-legacy",
        "question": "Pick",
        "options": [{ "id": "opt-0", "label": "A" }],
    }))
    .execute(&pool)
    .await
    .expect("insert legacy row");

    let channel = lookup_question_channel(&pool, thread_id, tool_use_id)
        .await
        .expect("lookup must succeed");
    assert!(
        channel.is_none(),
        "row without `channel` must surface as None so the caller can default to CC behaviour"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Pure-function decision: which channels trigger the CC-specific resume
/// side-effects? `ClaudeCode` (and absent, for back-compat) → yes; `Chat`
/// → no; trigger or other → no.
#[test]
fn should_emit_cc_resume_side_effects_only_for_claude_code_channel() {
    assert!(should_emit_cc_resume_side_effects(Some(
        EventChannel::ClaudeCode
    )));
    assert!(
        should_emit_cc_resume_side_effects(None),
        "legacy rows without a channel must default to today's CC behaviour"
    );
    assert!(!should_emit_cc_resume_side_effects(Some(
        EventChannel::Chat
    )));
    assert!(!should_emit_cc_resume_side_effects(Some(
        EventChannel::Trigger
    )));
}

/// Engine-restart-style abort: the user's "Restarted" exchange in the UI
/// is paired with this `ResponseAborted` row in the DB.
#[tokio::test]
async fn response_aborted_orphans_only_active_lookup() {
    assert_terminator_orphans_only_active_lookup(
        "toolu-orphaned-aborted",
        |bus, thread_id| async move {
            crate::engine::thread_events::emit_response_aborted(
                &bus,
                thread_id,
                crate::engine::thread_events::AbortCause::EngineShutdown,
                String::new(),
                vec![],
                None,
                None,
                cc_meta(),
                "[test] engine_shutdown",
            )
            .await;
        },
    )
    .await;
}



#[test]
fn selected_answer_resolves_to_label() {
    let answer = serde_json::json!({"kind": "Selected", "option_id": "opt-1"});
    let out = build_hook_answers(&[answer], &questions());
    assert_eq!(out, serde_json::json!({"Fav color?": "Blue"}));
}

#[test]
fn free_text_passes_through() {
    let answer = serde_json::json!({"kind": "FreeText", "text": "purple"});
    let out = build_hook_answers(&[answer], &questions());
    assert_eq!(out, serde_json::json!({"Fav color?": "purple"}));
}

#[test]
fn canceled_returns_explicit_marker_not_empty_object() {
    // Empty `{}` would be read as "unanswered" by CC's model, causing an
    // infinite re-invocation loop. The marker terminates the call.
    let answer = serde_json::json!({"kind": "Canceled"});
    let out = build_hook_answers(&[answer], &questions());
    assert_eq!(out, serde_json::json!({"Fav color?": "(canceled)"}));
}

#[test]
fn missing_label_falls_back_to_option_id() {
    let answer = serde_json::json!({"kind": "Selected", "option_id": "opt-9"});
    let out = build_hook_answers(&[answer], &questions());
    assert_eq!(out, serde_json::json!({"Fav color?": "opt-9"}));
}

#[test]
fn multi_selected_joins_labels_with_comma_space() {
    let answer = serde_json::json!({
        "kind": "MultiSelected",
        "option_ids": ["opt-0", "opt-1"]
    });
    let out = build_hook_answers(&[answer], &questions());
    assert_eq!(out, serde_json::json!({"Fav color?": "Red, Blue"}));
}

#[test]
fn multi_selected_unknown_id_falls_back_to_id() {
    let answer = serde_json::json!({
        "kind": "MultiSelected",
        "option_ids": ["opt-0", "opt-9"]
    });
    let out = build_hook_answers(&[answer], &questions());
    assert_eq!(out, serde_json::json!({"Fav color?": "Red, opt-9"}));
}

#[test]
fn multi_selected_single_id_yields_one_label() {
    let answer = serde_json::json!({
        "kind": "MultiSelected",
        "option_ids": ["opt-1"]
    });
    let out = build_hook_answers(&[answer], &questions());
    assert_eq!(out, serde_json::json!({"Fav color?": "Blue"}));
}

#[test]
fn multi_selected_with_text_appends_after_labels() {
    let answer = serde_json::json!({
        "kind": "MultiSelected",
        "option_ids": ["opt-0", "opt-1"],
        "text": "and also purple",
    });
    let out = build_hook_answers(&[answer], &questions());
    assert_eq!(
        out,
        serde_json::json!({"Fav color?": "Red, Blue, and also purple"})
    );
}

#[test]
fn multi_selected_with_only_text_yields_just_text() {
    let answer = serde_json::json!({
    "kind": "MultiSelected",
    "option_ids": [],
    "text": "freeform answer",
    });
    let out = build_hook_answers(&[answer], &questions());
    assert_eq!(out, serde_json::json!({"Fav color?": "freeform answer"}));
}

#[test]
fn multi_selected_with_empty_text_omits_trailing_separator() {
    // Empty `text` must NOT add a trailing ", " — the separator only
    // appears between non-empty parts.
    let answer = serde_json::json!({
        "kind": "MultiSelected",
        "option_ids": ["opt-0"],
        "text": "",
    });
    let out = build_hook_answers(&[answer], &questions());
    assert_eq!(out, serde_json::json!({"Fav color?": "Red"}));
}

#[test]
fn build_hook_answers_pairs_each_question_with_its_own_options() {
    // Per-question option ids restart at opt-0; the lookup must consult
    // the matching question's options array, not always questions[0].
    let answers = vec![
        serde_json::json!({"kind": "Selected", "option_id": "opt-1"}),
        serde_json::json!({"kind": "Selected", "option_id": "opt-0"}),
        serde_json::json!({
            "kind": "MultiSelected",
            "option_ids": ["opt-0", "opt-1"],
        }),
    ];
    let out = build_hook_answers(&answers, &three_questions());
    assert_eq!(
        out,
        serde_json::json!({
            "Fav color?": "Blue",
            "Fav animal?": "Cat",
            "Pick all toppings": "Cheese, Olives",
        })
    );
}

#[test]
fn build_hook_answers_pads_missing_answers_with_canceled_marker() {
    // Loop short-circuited on cancel after Q1 answered. Q2 + Q3 must
    // surface as `(canceled)` — never empty/missing keys, which CC reads
    // as "unanswered" and retries the whole tool call.
    let answers = vec![serde_json::json!({"kind": "Selected", "option_id": "opt-0"})];
    let out = build_hook_answers(&answers, &three_questions());
    assert_eq!(
        out,
        serde_json::json!({
            "Fav color?": "Red",
            "Fav animal?": "(canceled)",
            "Pick all toppings": "(canceled)",
        })
    );
}

#[test]
fn build_hook_answers_handles_zero_questions() {
    let out = build_hook_answers(&[], &serde_json::json!([]));
    assert_eq!(out, serde_json::json!({}));
}

#[test]
fn build_hook_answers_keys_on_question_field_ignoring_header() {
    // Strict contract: the answer-map key is the `question` field; `header`
    // is never used as a substitute (walk_question_batch rejects a missing
    // question upstream, so build_hook_answers only ever sees real questions).
    let questions = serde_json::json!([
        {"question": "Real question?", "header": "Chip", "options": [{"label": "Go"}, {"label": "Stop"}]},
    ]);
    let answers = vec![serde_json::json!({"kind": "Selected", "option_id": "opt-0"})];
    let out = build_hook_answers(&answers, &questions);
    assert_eq!(out, serde_json::json!({ "Real question?": "Go" }));
}

#[test]
fn lookup_option_label_uses_question_index_not_first() {
    let questions = three_questions();
    assert_eq!(
        lookup_option_label("opt-1", &questions, 1),
        "Dog",
        "must look up Q2's options[1], not Q1's"
    );
    assert_eq!(
        lookup_option_label("opt-0", &questions, 2),
        "Cheese",
        "must look up Q3's options[0]"
    );
}

#[test]
fn synth_question_id_format_is_outer_hash_q_index() {
    // The hash separator + `q` prefix must stay stable — the wait
    // registry, the per-question UserQuestionAsked emit, and the
    // crash-recovery answer lookup all key on this exact string.
    assert_eq!(synth_question_id("toolu_xyz", 0), "toolu_xyz#q0");
    assert_eq!(synth_question_id("toolu_xyz", 12), "toolu_xyz#q12");
}

#[test]
fn is_canceled_answer_only_matches_explicit_canceled_kind() {
    assert!(is_canceled_answer(&serde_json::json!({"kind": "Canceled"})));
    assert!(!is_canceled_answer(
        &serde_json::json!({"kind": "Selected", "option_id": "opt-0"})
    ));
    assert!(!is_canceled_answer(&serde_json::json!({})));
    assert!(!is_canceled_answer(&serde_json::Value::Null));
}

#[test]
fn build_hook_answers_disambiguates_duplicate_question_texts() {
    // CC's hook output is keyed by question text. If CC sends two
    // questions with identical text, a naive `Map::insert` would
    // overwrite the first answer — same UX failure as the bug this
    // feature fixes. Disambiguate with `" (#i)"` so every answer is
    // carried back to CC.
    let dupe_questions = serde_json::json!([
        {"question": "Pick one", "options": [{"label": "A"}]},
        {"question": "Pick one", "options": [{"label": "B"}]},
    ]);
    let answers = vec![
        serde_json::json!({"kind": "Selected", "option_id": "opt-0"}),
        serde_json::json!({"kind": "Selected", "option_id": "opt-0"}),
    ];
    let out = build_hook_answers(&answers, &dupe_questions);
    let obj = out.as_object().expect("object");
    assert_eq!(obj.len(), 2, "both answers must survive — got {out}");
    assert_eq!(obj.get("Pick one"), Some(&serde_json::json!("A")));
    assert_eq!(obj.get("Pick one (#2)"), Some(&serde_json::json!("B")));
}

/// `CodingAgentIdled` boundary: the synthetic idle the engine-restart
/// sweep emits alongside the abort. Filtering on idled too means an
/// unanswered question can't intercept follow-ups even if only the idle
/// boundary made it to the DB.
#[tokio::test]
async fn coding_agent_idled_orphans_only_active_lookup() {
    assert_terminator_orphans_only_active_lookup(
        "toolu-orphaned-idle",
        |bus, thread_id| async move {
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::CodingAgentIdled {
                    has_changes: false,
                    is_external_repo: false,
                    requires_restart: false,
                    cc_session_id: None,
                    coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                    reason: Some("engine_restart_interrupt".into()),
                    worktree_path: None,
                    worktree_head_sha: None,
                    bg_bash_pending: false,
                },
                meta: cc_meta(),
            })
            .await
            .expect("CodingAgentIdled emit")
            .expect("CodingAgentIdled persisted");
        },
    )
    .await;
}

// -- Progression-overtaken regression tests --------------------------------
// CC's parallel-tool-call race: AskUserQuestion is emitted alongside
// sibling tool_uses in one assistant message; the hook blocks the
// question, but the siblings dispatch concurrently and emit
// CodingAgent{TextStreamed,ToolCalled,ToolResult,…} while the question
// is still unanswered. Without progression-event filtering, the user's
// next typed text is absorbed as a FreeText answer to the dead question.

#[tokio::test]
async fn coding_agent_text_streamed_orphans_only_active_lookup() {
    assert_terminator_orphans_only_active_lookup(
        "toolu-overtaken-cc-text",
        |bus, thread_id| async move {
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::CodingAgentTextStreamed {
                    text: "carrying on with parallel work\n".into(),
                    coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                },
                meta: cc_meta(),
            })
            .await
            .expect("CodingAgentTextStreamed emit")
            .expect("CodingAgentTextStreamed persisted");
        },
    )
    .await;
}

#[tokio::test]
async fn coding_agent_tool_called_orphans_only_active_lookup() {
    assert_terminator_orphans_only_active_lookup(
        "toolu-overtaken-cc-tool",
        |bus, thread_id| async move {
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::CodingAgentToolCalled {
                    name: "Bash".into(),
                    args: serde_json::json!({"command": "ls"}),
                    description: String::new(),
                    coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                    tool_use_id: "toolu-sibling".into(),
                },
                meta: cc_meta(),
            })
            .await
            .expect("CodingAgentToolCalled emit")
            .expect("CodingAgentToolCalled persisted");
        },
    )
    .await;
}

#[tokio::test]
async fn second_user_question_replaces_first_as_active() {
    // When CC asks a second question while the first is still
    // unanswered, the active lookup naturally returns the LATEST one
    // (the SQL is `ORDER BY sequence DESC LIMIT 1`). The user's typed
    // text routes to the most recent question, which is what they're
    // visually answering. `UserQuestionAsked` is therefore NOT in the
    // overtaken set — including it would be redundant.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    emit_user_question(&bus, thread_id, "toolu-first").await;
    emit_user_question(&bus, thread_id, "toolu-second").await;

    assert_eq!(
        lookup_active_question_tool_use_id(&pool, thread_id)
            .await
            .as_deref(),
        Some("toolu-second"),
        "two unanswered questions: active lookup returns the latest (replacement)"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn chat_text_streamed_orphans_active_lookup_on_chat_thread() {
    // Chat symmetry: `TextStreamed` is the chat-agent variant. Today's
    // chat path can't actually emit progression past a question (the
    // agentic loop blocks sequentially on `ask_user_question`), but the
    // overtaken set is uniform across agents so a future regression
    // can't reintroduce the bug shape.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    // Chat threads bootstrap on MessageReceived, not SessionStarted.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "ask me something".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: crate::engine::thread_events::ActorMode::Human,
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
    .expect("MessageReceived emit")
    .expect("MessageReceived persisted");

    let tool_use_id = "toolu-chat-overtaken";
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.into(),
            cc_session_id: String::new(),
            question: "Pick".into(),
            options: vec![opt("opt-0", "A")],
            worktree_path: None,
            multi_select: false,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("UserQuestionAsked emit")
    .expect("UserQuestionAsked persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TextStreamed {
            text: "chat-agent kept talking\n".into(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("TextStreamed emit")
    .expect("TextStreamed persisted");

    assert!(
        lookup_active_question_tool_use_id(&pool, thread_id)
            .await
            .is_none(),
        "chat TextStreamed after UserQuestionAsked must orphan the active lookup"
    );
    assert_eq!(
        lookup_pending_question_tool_use_id(&pool, thread_id)
            .await
            .as_deref(),
        Some(tool_use_id),
        "broad lookup still surfaces the orphan so archive can cancel-stamp"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
