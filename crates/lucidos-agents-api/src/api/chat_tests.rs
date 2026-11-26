use super::*;
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::thread_events::ThreadEvent;

#[tokio::test]
async fn stop_restores_before_canceling_a_live_turn_and_still_cancels_on_restore_failure() {
    use crate::engine::event_bus::OrdinarySubmissionIntent;
    use crate::engine::thread_events::MessageOrigin;
    use crate::engine::thread_queue::ThreadQueueRequest;
    use crate::test_support::*;
    if !in_engine_child("api::chat::tests::stop_restores_before_canceling_a_live_turn_and_still_cancels_on_restore_failure").await {
        return;
    }
    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = crate::api::permission_requests::tests::permission_test_state(
        engine.clone(),
        workspace.path().to_path_buf(),
    )
    .await;
    let bus = &engine.event_bus;
    crate::core::DeviceStore::register(&pool, bus, "stop-browser", None, None, None)
        .await
        .unwrap();
    for full in [false, true] {
        let thread = seed_chat_thread(bus, "active turn").await;
        let message = Uuid::new_v4();
        let actor = MessageOrigin::Device {
            device_id: "stop-browser".into(),
        };
        let request: ThreadQueueRequest = serde_json::from_value(serde_json::json!({
            "type":"agent-chat", "thread_id":thread, "event_id":message,
            "message":"unread input", "mode":"human", "origin":actor,
        }))
        .unwrap();
        let intent = OrdinarySubmissionIntent::new(thread, message, &request).unwrap();
        bus.accept_ordinary_submission(request, actor, &bus.input_order, None, None, intent)
            .await
            .unwrap();
        if full {
            sqlx::query("UPDATE thread_summaries SET compose_text=$2 WHERE thread_id=$1")
                .bind(thread)
                .bind("x".repeat(crate::core::store::MAX_COMPOSE_TEXT_BYTES))
                .execute(&pool)
                .await
                .unwrap();
        }
        let (token, _injections, guard) = engine.try_register_thread(thread).unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            crate::core::subprocess_origin::HEADER_DEVICE_ID,
            "stop-browser".parse().unwrap(),
        );
        let Json(response) = cancel_chat(
            State(state.clone()),
            headers,
            Query(CancelChatQuery {
                thread_id: Some(thread.to_string()),
            }),
        )
        .await
        .unwrap();
        assert!(response.canceled);
        assert!(
            token.is_cancelled(),
            "the live runtime token must fire even when restoration fails"
        );
        assert_eq!(
            response.take_back.failed_take_back_ids,
            if full { vec![message] } else { vec![] }
        );
        let removed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE thread_id=$1 AND event_type='QueuedMessageRemoved'",
        )
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(removed, i64::from(!full));
        if !full {
            assert_eq!(
                response.take_back.taken_back_messages[0].compose.text,
                "unread input"
            );
        }
        drop(guard);
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn subprocess_chat_rejects_human_mode_targeting_any_thread() {
    let source = Some(Uuid::new_v4());
    let target_other = Some(Uuid::new_v4());
    assert!(!subprocess_chat_legitimate(
        ActorMode::Human,
        source,
        target_other,
        None,
        true,
    ));
    assert!(!subprocess_chat_legitimate(
        ActorMode::Human,
        source,
        source,
        None,
        true,
    ));
    assert!(!subprocess_chat_legitimate(
        ActorMode::Human,
        source,
        None,
        None,
        false,
    ));
}

#[test]
fn subprocess_chat_rejects_cross_thread_agent_post() {
    let source = Some(Uuid::new_v4());
    let target_other = Some(Uuid::new_v4());
    assert!(!subprocess_chat_legitimate(
        ActorMode::Agent,
        source,
        target_other,
        None,
        true,
    ));
    assert!(!subprocess_chat_legitimate(
        ActorMode::Engine,
        source,
        target_other,
        None,
        true,
    ));
}

#[test]
fn subprocess_chat_allows_same_thread_agent_followup() {
    let source = Some(Uuid::new_v4());
    assert!(subprocess_chat_legitimate(
        ActorMode::Agent,
        source,
        source,
        None,
        true,
    ));
    assert!(subprocess_chat_legitimate(
        ActorMode::Engine,
        source,
        source,
        None,
        true,
    ));
}

#[test]
fn subprocess_chat_allows_spawn_with_matching_parent_thread() {
    let source = Some(Uuid::new_v4());
    assert!(subprocess_chat_legitimate(
        ActorMode::Agent,
        source,
        None,
        source,
        false,
    ));
}

#[test]
fn subprocess_chat_rejects_spawn_with_mismatched_parent_thread() {
    let source = Some(Uuid::new_v4());
    let lied_parent = Some(Uuid::new_v4());
    assert!(!subprocess_chat_legitimate(
        ActorMode::Agent,
        source,
        None,
        lied_parent,
        false,
    ));
}

#[test]
fn parent_matches_source_requires_a_new_target() {
    let source = Some(Uuid::new_v4());
    let someone_elses_thread = Some(Uuid::new_v4());

    for mode in [ActorMode::Agent, ActorMode::Engine] {
        assert!(
            !subprocess_chat_legitimate(mode, source, someone_elses_thread, source, true),
            "{mode:?}: claiming parenthood must not open an EXISTING thread"
        );
        assert!(
            subprocess_chat_legitimate(mode, source, someone_elses_thread, source, false),
            "{mode:?}: a spawn naming its own new thread id stays allowed"
        );
    }
}

#[test]
fn spawn_thread_with_a_pregenerated_id_is_still_allowed() {
    let source = Some(Uuid::new_v4());
    let pregenerated = Some(Uuid::new_v4());
    assert!(subprocess_chat_legitimate(
        ActorMode::Agent,
        source,
        pregenerated,
        source,
        false,
    ));
}

#[test]
fn target_matches_source_is_unaffected_by_the_new_conjunct() {
    let source = Some(Uuid::new_v4());
    assert!(subprocess_chat_legitimate(
        ActorMode::Agent,
        source,
        source,
        None,
        true,
    ));
    assert!(subprocess_chat_legitimate(
        ActorMode::Agent,
        source,
        source,
        source,
        true,
    ));
}

#[test]
fn subprocess_chat_no_source_thread_still_rejects_cross_thread() {
    let target_other = Some(Uuid::new_v4());
    assert!(!subprocess_chat_legitimate(
        ActorMode::Agent,
        None,
        target_other,
        None,
        true,
    ));
}

#[test]
fn unattributed_human_mode_is_refused() {
    assert!(!human_mode_is_attributed(ActorMode::Human, false, false));
}

#[test]
fn a_registered_device_attributes_a_human() {
    assert!(human_mode_is_attributed(ActorMode::Human, true, false));
}

#[test]
fn a_cross_workspace_caller_may_still_speak_for_its_human() {
    assert!(human_mode_is_attributed(ActorMode::Human, false, true));
}

#[test]
fn agent_and_engine_modes_need_no_human_evidence() {
    for mode in [ActorMode::Agent, ActorMode::Engine] {
        assert!(
            human_mode_is_attributed(mode, false, false),
            "mode: {mode:?}"
        );
    }
}

#[test]
fn dropping_the_origin_token_never_buys_more_than_presenting_it() {
    let source = Uuid::new_v4();
    let other = Uuid::new_v4();
    for (label, target, parent, target_exists) in [
        ("cross-thread, existing target", Some(other), None, true),
        ("own thread", Some(source), None, true),
        ("no target", None, None, false),
        ("claimed spawn", Some(other), Some(source), false),
    ] {
        let with_token = subprocess_chat_legitimate(
            ActorMode::Human,
            Some(source),
            target,
            parent,
            target_exists,
        );
        let without_token = human_mode_is_attributed(ActorMode::Human, false, false);
        assert!(
            !with_token,
            "{label}: the token-present path must refuse mode=human"
        );
        assert!(
            !without_token,
            "{label}: the token-absent path must not be more permissive"
        );
    }
}

#[test]
fn the_human_refusal_tells_the_agent_what_to_do_instead() {
    let msg = HUMAN_MODE_UNATTRIBUTED;
    assert!(
        msg.contains("registered device"),
        "names the evidence: {msg}"
    );
    assert!(
        msg.contains("follow_up_child_thread"),
        "names the legitimate route: {msg}"
    );
    assert!(
        msg.contains("tell the user it is not possible"),
        "names what to do when no tool covers the request: {msg}"
    );
}

#[test]
fn an_existing_thread_is_always_addressable() {
    assert!(thread_target_is_addressable(true, false, None, None, false));
}

#[test]
fn an_unknown_thread_with_no_create_signal_is_refused() {
    assert!(!thread_target_is_addressable(
        false, false, None, None, false
    ));
    assert!(!thread_target_is_addressable(
        false,
        false,
        Some(false),
        None,
        false
    ));
}

#[test]
fn each_create_signal_admits_an_unknown_thread() {
    assert!(thread_target_is_addressable(
        false,
        false,
        Some(true),
        None,
        false
    ));
    assert!(thread_target_is_addressable(
        false,
        false,
        None,
        Some(Uuid::new_v4()),
        false
    ));
    assert!(thread_target_is_addressable(false, false, None, None, true));
}

#[test]
fn an_unknown_thread_with_an_admitted_input_is_addressable() {
    assert!(thread_target_is_addressable(false, true, None, None, false));
}

#[test]
fn the_unknown_thread_refusal_points_at_the_wrong_engine() {
    let tid = Uuid::new_v4();
    let msg = unknown_thread_message(tid);
    assert!(msg.contains(&tid.to_string()), "names the id: {msg}");
    assert!(msg.contains("new_thread"), "names the create signal: {msg}");
    assert!(
        msg.contains("nothing was written"),
        "says the write did not happen: {msg}"
    );
    assert!(
        msg.contains("/api/v1/health"),
        "points at the way to check which engine answered: {msg}"
    );
}

#[test]
fn a_create_with_no_text_and_no_image_is_refused() {
    for blank in ["", "  \n\t "] {
        let err = require_input_to_create(false, blank, &[]).expect_err("nothing to start with");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.message, NOTHING_TO_START_THE_THREAD_WITH);
    }
}

#[test]
fn an_image_alone_is_enough_to_create() {
    let image = ChatImage {
        base64: "AA==".into(),
        mime_type: "image/png".into(),
    };
    assert!(require_input_to_create(false, "", std::slice::from_ref(&image)).is_ok());
}

#[test]
fn text_alone_is_enough_to_create() {
    assert!(require_input_to_create(false, "do the thing", &[]).is_ok());
}

#[test]
fn a_drafts_first_send_is_a_create() {
    assert!(!thread_has_started(None));
    assert!(!thread_has_started(Some(ThreadState::Composing.as_str())));
    assert!(thread_has_started(Some(ThreadState::Active.as_str())));
}

#[test]
fn a_follow_up_is_not_judged_here() {
    assert!(require_input_to_create(true, "", &[]).is_ok());
}

#[test]
fn a_new_coding_agent_thread_with_an_unoffered_model_is_refused() {
    let err = require_offered_coding_agent_model(false, Some(true), None, Some("claude-sonnet-9"))
        .expect_err("an id no list offers must fail the create");
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        Err(err.message),
        crate::runtime::validate_coding_agent_model(
            crate::runtime::CodingAgent::ClaudeCode,
            Some("claude-sonnet-9")
        ),
        "the route and the tool refuse with one text"
    );
}

#[test]
fn the_route_checks_the_model_against_the_requested_backend() {
    let err = require_offered_coding_agent_model(
        false,
        Some(true),
        Some(crate::runtime::CodingAgent::Codex),
        Some("claude-sonnet-5"),
    )
    .expect_err("a Claude id is not a Codex model");
    assert!(err.message.contains("codex"), "{}", err.message);
}

#[test]
fn a_follow_up_on_a_started_thread_keeps_an_unlisted_model() {
    assert!(
        require_offered_coding_agent_model(true, Some(true), None, Some("claude-sonnet-9")).is_ok()
    );
}

#[test]
fn a_model_on_a_chat_agent_request_is_not_judged_here() {
    for use_coding_agent in [None, Some(false)] {
        assert!(require_offered_coding_agent_model(
            false,
            use_coding_agent,
            None,
            Some("claude-sonnet-9")
        )
        .is_ok());
    }
}

#[test]
fn an_offered_or_absent_model_passes() {
    for model in [None, Some(""), Some("claude-opus-5-5[1m]"), Some("haiku")] {
        assert!(
            require_offered_coding_agent_model(false, Some(true), None, model).is_ok(),
            "{model:?}"
        );
    }
}

#[test]
fn the_tool_the_route_and_the_picker_share_one_list() {
    use crate::runtime::{coding_agent_model_options, validate_coding_agent_model, CodingAgent};
    let offered = coding_agent_model_options(CodingAgent::ClaudeCode);
    let tool = validate_coding_agent_model(CodingAgent::ClaudeCode, Some("claude-sonnet-9"))
        .expect_err("no row offers it");
    let route =
        require_offered_coding_agent_model(false, Some(true), None, Some("claude-sonnet-9"))
            .expect_err("the route refuses the same id");
    assert_eq!(route.message, tool);

    let listed = tool
        .split_once("Choose one of: ")
        .expect("the refusal lists the offered rows")
        .1;
    let refused_ids: Vec<&str> = listed
        .split(", ")
        .map(|row| row.split(" (runs ").next().unwrap_or(row))
        .collect();
    let offered_ids: Vec<&str> = offered.iter().map(|o| o.value.as_str()).collect();
    assert_eq!(refused_ids, offered_ids);

    let definitions = crate::runtime::claude_code::cc_command_definitions();
    let served = definitions[0]["params"][0]["options"]
        .as_array()
        .expect("set_model has options");
    for row in served {
        let value = row["value"].as_str().expect("a served row has a value");
        assert!(
            offered_ids.contains(&value),
            "the picker serves {value}, which is not offered"
        );
    }
}

fn base_req(mode: ActorMode) -> ChatRequest {
    ChatRequest {
        message: "hi".into(),
        model: None,
        app_context: None,
        file_context: None,
        url_context: None,
        repo_file_context: None,
        reasoning_effort: None,
        provider: None,
        images: None,
        image_hashes: None,
        device_id: None,
        use_coding_agent: None,
        coding_agent: None,
        cc_model: None,
        event_id: None,
        thread_id: None,
        input_chain: None,
        input_seq: None,
        compose_epoch: None,
        new_thread: None,
        conflict_change_id: None,
        repo_id: None,
        folder: None,
        title: None,
        mode,
        parent_thread_id: None,
        spawning_event_id: None,
        wake_mode: None,
        caller_workspace: None,
        caller_thread_id: None,
        caller_event_id: None,
        integrated: false,
        mentions: vec![],
        agent_mentioned: false,
    }
}

#[test]
fn a_wake_mode_needs_a_parent_and_a_known_name() {
    let mut req = base_req(ActorMode::Agent);
    req.parent_thread_id = Some(Uuid::new_v4().to_string());
    req.wake_mode = Some("wake-when-all-done".into());
    let v = validate_mode_and_spawn(&req).unwrap();
    assert_eq!(v.declared_wake_mode, Some(WakeMode::WakeWhenAllDone));

    req.wake_mode = Some("file".into());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);

    let mut top = base_req(ActorMode::Agent);
    top.caller_workspace = Some("dev".into());
    top.wake_mode = Some("wake".into());
    assert_eq!(spawn_refusal(&top), StatusCode::BAD_REQUEST);
}

fn spawn_refusal(request: &ChatRequest) -> StatusCode {
    validate_mode_and_spawn(request)
        .expect_err("expected a refusal")
        .status
}

#[test]
fn human_mode_with_no_spawn_context_is_valid() {
    let req = base_req(ActorMode::Human);
    let v = validate_mode_and_spawn(&req).unwrap();
    assert_eq!(v.parent_thread_id, None);
    assert_eq!(v.spawning_event_id, None);
}

#[test]
fn human_mode_with_parent_thread_id_returns_400() {
    let mut req = base_req(ActorMode::Human);
    req.parent_thread_id = Some(Uuid::new_v4().to_string());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn human_mode_with_spawning_event_id_returns_400() {
    let mut req = base_req(ActorMode::Human);
    req.spawning_event_id = Some(Uuid::new_v4().to_string());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn agent_mode_with_parent_and_spawning_event_is_valid() {
    let parent_uuid = Uuid::new_v4();
    let event_uuid = Uuid::new_v4();
    let mut req = base_req(ActorMode::Agent);
    req.parent_thread_id = Some(parent_uuid.to_string());
    req.spawning_event_id = Some(event_uuid.to_string());
    let v = validate_mode_and_spawn(&req).unwrap();
    assert_eq!(v.parent_thread_id, Some(parent_uuid));
    assert_eq!(v.spawning_event_id, Some(event_uuid));
}

#[test]
fn agent_mode_without_parent_thread_id_returns_400() {
    let req = base_req(ActorMode::Agent);
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn engine_mode_without_parent_thread_id_returns_400() {
    let req = base_req(ActorMode::Engine);
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn engine_mode_with_caller_workspace_is_valid() {
    let mut req = base_req(ActorMode::Engine);
    req.caller_workspace = Some("dev".into());
    assert!(validate_mode_and_spawn(&req).is_ok());
}

#[test]
fn invalid_parent_uuid_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.parent_thread_id = Some("not-a-uuid".into());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn invalid_spawning_event_uuid_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.parent_thread_id = Some(Uuid::new_v4().to_string());
    req.spawning_event_id = Some("not-a-uuid".into());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn chat_request_requires_mode() {
    let body = serde_json::json!({
        "message": "spawned task",
        "use_coding_agent": true,
    });
    let result: Result<ChatRequest, _> = serde_json::from_value(body);
    assert!(result.is_err(), "ChatRequest must require mode field");
}

#[test]
fn chat_request_mode_field_deserializes() {
    let body = serde_json::json!({
        "message": "hi",
        "mode": "human",
    });
    let req: ChatRequest = serde_json::from_value(body).unwrap();
    assert_eq!(req.mode, ActorMode::Human);
}

#[test]
fn chat_request_accepts_legacy_use_claude_code_alias() {
    let body = serde_json::json!({
        "message": "spawned task",
        "mode": "agent",
        "use_claude_code": true,
    });
    let req: ChatRequest = serde_json::from_value(body).unwrap();
    assert_eq!(req.use_coding_agent, Some(true));
}

#[test]
fn caller_workspace_with_parent_thread_id_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("dev".into());
    req.parent_thread_id = Some(Uuid::new_v4().to_string());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn caller_workspace_with_spawning_event_id_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("dev".into());
    req.spawning_event_id = Some(Uuid::new_v4().to_string());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn caller_thread_id_without_caller_workspace_returns_400() {
    let mut req = base_req(ActorMode::Human);
    req.caller_thread_id = Some(Uuid::new_v4().to_string());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn caller_event_id_without_caller_workspace_returns_400() {
    let mut req = base_req(ActorMode::Human);
    req.caller_event_id = Some(Uuid::new_v4().to_string());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn caller_thread_id_invalid_uuid_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("dev".into());
    req.caller_thread_id = Some("not-a-uuid".into());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn caller_event_id_invalid_uuid_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("dev".into());
    req.caller_event_id = Some("not-a-uuid".into());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn caller_workspace_with_only_workspace_field_is_valid() {
    let mut req = base_req(ActorMode::Human);
    req.caller_workspace = Some("dev".into());
    let v = validate_mode_and_spawn(&req).unwrap();
    assert_eq!(v.parent_thread_id, None);
    assert_eq!(v.spawning_event_id, None);
}

#[test]
fn caller_workspace_with_all_three_fields_is_valid() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("dev".into());
    req.caller_thread_id = Some(Uuid::new_v4().to_string());
    req.caller_event_id = Some(Uuid::new_v4().to_string());
    assert!(validate_mode_and_spawn(&req).is_ok());
}

#[test]
fn a_thread_may_not_be_its_own_parent() {
    let tid = Uuid::new_v4();
    let mut req = base_req(ActorMode::Agent);
    req.thread_id = Some(tid.to_string());
    req.parent_thread_id = Some(tid.to_string());

    let err = validate_mode_and_spawn(&req).expect_err("self-parent must be refused");
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(
        err.message.contains("parent_thread_id"),
        "names the offending field: {}",
        err.message
    );
    assert!(
        err.message.contains("own parent"),
        "names the problem, not just the status: {}",
        err.message
    );
}

#[test]
fn the_self_parent_check_compares_uuids_not_strings() {
    let tid = Uuid::new_v4();
    let mut req = base_req(ActorMode::Agent);
    req.thread_id = Some(tid.to_string().to_uppercase());
    req.parent_thread_id = Some(tid.to_string());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn a_parent_that_differs_from_the_target_is_still_accepted() {
    let tid = Uuid::new_v4();
    let parent = Uuid::new_v4();
    let mut req = base_req(ActorMode::Agent);
    req.thread_id = Some(tid.to_string());
    req.parent_thread_id = Some(parent.to_string());

    let v = validate_mode_and_spawn(&req).unwrap();
    assert_eq!(v.thread_id, Some(tid));
    assert_eq!(v.parent_thread_id, Some(parent));
}

#[test]
fn a_target_with_no_parent_is_not_read_as_self_parented() {
    let mut req = base_req(ActorMode::Human);
    req.thread_id = None;
    req.parent_thread_id = None;
    assert!(validate_mode_and_spawn(&req).is_ok());
}

#[test]
fn a_malformed_target_thread_id_is_still_a_400() {
    let mut req = base_req(ActorMode::Human);
    req.thread_id = Some("not-a-uuid".into());
    assert_eq!(spawn_refusal(&req), StatusCode::BAD_REQUEST);
}

#[test]
fn continuity_new_thread_is_always_ok() {
    assert!(validate_thread_continuity(None, None, None, None, None, None).is_ok());
    assert!(validate_thread_continuity(None, None, None, Some(true), Some("repo-a"), None).is_ok());
}

#[test]
fn continuity_chat_thread_with_chat_followup_is_ok() {
    assert!(validate_thread_continuity(Some("chat"), None, None, None, None, None).is_ok());
    assert!(validate_thread_continuity(Some("chat"), None, None, Some(false), None, None).is_ok());
}

#[test]
fn continuity_chat_thread_rejects_cc_followup() {
    let err =
        validate_thread_continuity(Some("chat"), None, None, Some(true), Some("repo-a"), None)
            .unwrap_err();
    assert_eq!(err.0, StatusCode::CONFLICT);
    assert!(err.1.contains("Lucidos"));
    assert!(err.1.contains("coding-agent"));
}

#[test]
fn continuity_cc_thread_rejects_chat_followup() {
    let err =
        validate_thread_continuity(Some("claude_code"), Some("repo-a"), None, None, None, None)
            .unwrap_err();
    assert_eq!(err.0, StatusCode::CONFLICT);
    let err = validate_thread_continuity(
        Some("claude_code"),
        Some("repo-a"),
        None,
        Some(false),
        None,
        None,
    )
    .unwrap_err();
    assert_eq!(err.0, StatusCode::CONFLICT);
}

#[test]
fn continuity_cc_thread_with_matching_repo_is_ok() {
    assert!(validate_thread_continuity(
        Some("claude_code"),
        Some("repo-a"),
        None,
        Some(true),
        Some("repo-a"),
        None,
    )
    .is_ok());
}

#[test]
fn continuity_cc_thread_rejects_different_repo() {
    let err = validate_thread_continuity(
        Some("claude_code"),
        Some("repo-a"),
        None,
        Some(true),
        Some("repo-b"),
        None,
    )
    .unwrap_err();
    assert_eq!(err.0, StatusCode::CONFLICT);
    assert!(err.1.contains("repo-a"));
    assert!(err.1.contains("repo-b"));
}

#[test]
fn continuity_cc_thread_with_no_request_repo_is_ok() {
    assert!(validate_thread_continuity(
        Some("claude_code"),
        Some("repo-a"),
        None,
        Some(true),
        None,
        None
    )
    .is_ok());
}

#[test]
fn continuity_cc_thread_with_no_existing_repo_is_ok() {
    assert!(validate_thread_continuity(
        Some("claude_code"),
        None,
        None,
        Some(true),
        Some("repo-b"),
        None
    )
    .is_ok());
}

#[test]
fn continuity_cc_thread_rejects_backend_flip() {
    let err = validate_thread_continuity(
        Some("claude_code"),
        None,
        Some("claude-code"),
        Some(true),
        None,
        Some(crate::runtime::CodingAgent::Codex),
    )
    .unwrap_err();
    assert_eq!(err.0, StatusCode::CONFLICT);
    assert!(err.1.contains("codex"));
    assert!(err.1.contains("claude-code"));

    let err = validate_thread_continuity(
        Some("claude_code"),
        None,
        None,
        Some(true),
        None,
        Some(crate::runtime::CodingAgent::Codex),
    )
    .unwrap_err();
    assert_eq!(err.0, StatusCode::CONFLICT);
}

#[test]
fn continuity_cc_thread_accepts_matching_backend() {
    assert!(validate_thread_continuity(
        Some("claude_code"),
        None,
        Some("codex"),
        Some(true),
        None,
        Some(crate::runtime::CodingAgent::Codex),
    )
    .is_ok());
    assert!(validate_thread_continuity(
        Some("claude_code"),
        None,
        Some("codex"),
        Some(true),
        None,
        None,
    )
    .is_ok());
}

#[test]
fn continuity_trigger_thread_treated_as_chat() {
    let err = validate_thread_continuity(Some("trigger"), None, None, Some(true), None, None)
        .unwrap_err();
    assert_eq!(err.0, StatusCode::CONFLICT);
}

fn make_orphan(text: &str) -> InjectedPrompt {
    InjectedPrompt {
        text: text.to_string(),
        event_id: Some(Uuid::new_v4()),
        mode: ActorMode::Human,
        spawning_event_id: None,
        images: None,
        origin: None,
        kind: crate::engine::InjectedPromptKind::UserText,
    }
}

fn make_reentry(text: &str) -> InjectedPrompt {
    InjectedPrompt {
        text: text.to_string(),
        event_id: Some(Uuid::new_v4()),
        mode: ActorMode::Agent,
        spawning_event_id: Some(Uuid::new_v4()),
        images: None,
        origin: None,
        kind: crate::engine::InjectedPromptKind::ReentryFromEngine,
    }
}

#[tokio::test]
async fn drain_orphan_queue_coalesces_contiguous_user_orphans_in_order() {
    let initial = vec![make_orphan("a"), make_orphan("b"), make_orphan("c")];
    let processed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let processed_clone = processed.clone();
    drain_orphan_queue(initial, move |orphans| {
        let processed = processed_clone.clone();
        async move {
            processed
                .lock()
                .unwrap()
                .push(orphans.into_iter().map(|orphan| orphan.text).collect());
            Vec::new()
        }
    })
    .await;
    assert_eq!(
        *processed.lock().unwrap(),
        vec![vec!["a".to_string(), "b".to_string(), "c".to_string()]]
    );
}

#[tokio::test]
async fn drain_orphan_queue_keeps_an_engine_reentry_separate() {
    let initial = vec![make_orphan("a"), make_reentry("reentry"), make_orphan("b")];
    let processed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let processed_clone = processed.clone();
    drain_orphan_queue(initial, move |orphans| {
        let processed = processed_clone.clone();
        async move {
            processed
                .lock()
                .unwrap()
                .push(orphans.into_iter().map(|orphan| orphan.text).collect());
            Vec::new()
        }
    })
    .await;
    assert_eq!(
        *processed.lock().unwrap(),
        vec![
            vec!["a".to_string()],
            vec!["reentry".to_string()],
            vec!["b".to_string()]
        ]
    );
}

#[tokio::test]
async fn drain_orphan_queue_processes_orphans_of_orphans() {
    let initial = vec![make_orphan("a")];
    let processed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let processed_clone = processed.clone();
    drain_orphan_queue(initial, move |orphans| {
        let processed = processed_clone.clone();
        async move {
            assert_eq!(orphans.len(), 1);
            let text = orphans[0].text.clone();
            processed.lock().unwrap().push(text.clone());
            if text == "a" {
                vec![make_orphan("b")]
            } else {
                Vec::new()
            }
        }
    })
    .await;
    assert_eq!(*processed.lock().unwrap(), vec!["a", "b"]);
}

#[tokio::test]
async fn drain_orphan_queue_handles_deep_chains() {
    let initial = vec![make_orphan("0")];
    let processed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let processed_clone = processed.clone();
    drain_orphan_queue(initial, move |orphans| {
        let processed = processed_clone.clone();
        async move {
            assert_eq!(orphans.len(), 1);
            let text = orphans.into_iter().next().unwrap().text;
            let n: i32 = text.parse().unwrap();
            processed.lock().unwrap().push(text);
            if n < 4 {
                vec![make_orphan(&(n + 1).to_string())]
            } else {
                Vec::new()
            }
        }
    })
    .await;
    assert_eq!(*processed.lock().unwrap(), vec!["0", "1", "2", "3", "4"]);
}

#[tokio::test]
async fn drain_orphan_queue_no_op_for_empty_initial() {
    let processed = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let processed_clone = processed.clone();
    drain_orphan_queue(vec![], move |_orphans| {
        let processed = processed_clone.clone();
        async move {
            *processed.lock().unwrap() += 1;
            Vec::new()
        }
    })
    .await;
    assert_eq!(*processed.lock().unwrap(), 0);
}

#[tokio::test]
async fn the_removal_lookup_finds_only_its_own_tombstone() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _cb_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let kept_message_id = Uuid::new_v4();
    let removed_message_id = Uuid::new_v4();
    persist_message(&bus, thread_id, "kept", kept_message_id).await;

    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::QueuedMessageRemoved {
            removed_message_id,
            restore_to_compose: false,
            compose: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("QueuedMessageRemoved must persist");

    let was_removed =
        |message| crate::engine::consumption_claim::was_removed(&pool, thread_id, message);
    assert!(was_removed(removed_message_id).await.unwrap());
    assert!(!was_removed(kept_message_id).await.unwrap());

    crate::test_support::teardown_test_db(&db_name).await;
}

async fn persist_message(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    text: &str,
    event_id: Uuid,
) {
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            voice_session_id: None,
            wake_mode: None,
            text: text.into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            provider: None,
            origin: None,
        },
        meta: EventMeta {
            event_id: Some(event_id),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("MessageReceived must persist");
}

fn orphan_naming(event_id: Uuid, text: &str) -> InjectedPrompt {
    InjectedPrompt {
        event_id: Some(event_id),
        ..make_orphan(text)
    }
}

#[tokio::test]
async fn announce_orphan_batch_revives_a_thread_the_cancel_left_idle() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _cb_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let msg_a = Uuid::new_v4();
    let msg_b = Uuid::new_v4();

    persist_message(&bus, thread_id, "summarize the report", msg_a).await;
    persist_message(&bus, thread_id, "actually, just the totals", msg_b).await;
    crate::engine::thread_events::emit_response_canceled(
        &bus,
        &pool,
        thread_id,
        crate::engine::thread_events::CancelCause::UserStop,
        String::new(),
        vec![],
        None,
        None,
        EventMeta {
            request_event_id: Some(msg_a),
            ..EventMeta::NONE
        },
        "[Test] user stop",
    )
    .await;

    let idled: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("query");
    assert_eq!(
        idled, "idle",
        "premise of the bug: the Stop settles the thread even though a follow-up is queued behind it"
    );

    announce_orphan_batch(
        &bus,
        thread_id,
        vec![orphan_naming(msg_b, "actually, just the totals")],
    )
    .await;

    let revived: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("query");
    assert_eq!(
        revived, "running",
        "the re-submitted follow-up must read as running; an idle projection makes a working \
         thread look finished until its answer lands"
    );

    let named: Vec<String> = sqlx::query_scalar(
        "SELECT payload->>'injected_message_id' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'PromptInjected' ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .expect("query");
    assert_eq!(
        named,
        vec![msg_b.to_string()],
        "the announcement must name the follow-up's own MessageReceived, which is what the \
         client absorbs it into instead of rendering a duplicate panel"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn announce_orphan_batch_announces_every_orphan_in_the_batch() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _cb_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

    let mut batch = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        persist_message(&bus, thread_id, &format!("follow-up {i}"), *id).await;
        batch.push(orphan_naming(*id, &format!("follow-up {i}")));
    }

    announce_orphan_batch(&bus, thread_id, batch).await;

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT payload->>'injected_message_id', payload->>'request_event_id' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'PromptInjected' ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .expect("query");

    let named: Vec<String> = rows.iter().map(|(named, _)| named.clone()).collect();
    assert_eq!(
        named,
        ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
        "one announcement per orphan, in batch order"
    );
    assert!(
        rows.iter().all(|(_, anchor)| *anchor == ids[0].to_string()),
        "the whole batch runs as one turn, anchored on the first orphan's message"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn announce_orphan_batch_leaves_out_an_orphan_the_user_took_back() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _cb_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let ids: Vec<Uuid> = (0..2).map(|_| Uuid::new_v4()).collect();
    let mut batch = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        persist_message(&bus, thread_id, &format!("follow-up {i}"), *id).await;
        batch.push(orphan_naming(*id, &format!("follow-up {i}")));
    }
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::QueuedMessageRemoved {
            removed_message_id: ids[0],
            restore_to_compose: false,
            compose: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("QueuedMessageRemoved must persist");

    let announced = announce_orphan_batch(&bus, thread_id, batch).await;

    assert_eq!(
        announced.iter().map(|o| o.event_id).collect::<Vec<_>>(),
        vec![Some(ids[1])]
    );
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT payload->>'injected_message_id', payload->>'request_event_id' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'PromptInjected'",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .expect("query");
    assert_eq!(rows, vec![(ids[1].to_string(), ids[1].to_string())]);

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn announce_orphan_batch_is_a_noop_for_an_empty_batch() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _cb_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    announce_orphan_batch(&bus, thread_id, Vec::new()).await;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'PromptInjected'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("query");
    assert_eq!(count, 0);

    crate::test_support::teardown_test_db(&db_name).await;
}

#[test]
fn file_ctx_carries_a_data_file_line_range() {
    let ctx = FileContext {
        path: "artifacts/notes.md".to_string(),
        lines: Some((10, 20)),
    };
    assert_eq!(
        resolve_file_ctx(Some(&ctx), None).as_deref(),
        Some("artifacts/notes.md:10-20")
    );
}

#[test]
fn file_ctx_is_the_bare_path_when_nothing_is_selected() {
    let ctx = FileContext {
        path: "artifacts/notes.md".to_string(),
        lines: None,
    };
    assert_eq!(
        resolve_file_ctx(Some(&ctx), None).as_deref(),
        Some("artifacts/notes.md")
    );
}

#[test]
fn repo_file_ctx_keeps_its_repo_prefix_and_range() {
    let ctx = RepoFileContext {
        repo_id: "repo-1".to_string(),
        path: "src/main.rs".to_string(),
        lines: Some((510, 520)),
    };
    assert_eq!(
        resolve_file_ctx(None, Some(&ctx)).as_deref(),
        Some("[repo:repo-1] src/main.rs:510-520")
    );
}

#[test]
fn file_ctx_wins_over_repo_file_ctx_and_neither_is_none() {
    let file = FileContext {
        path: "artifacts/notes.md".to_string(),
        lines: None,
    };
    let repo = RepoFileContext {
        repo_id: "repo-1".to_string(),
        path: "src/main.rs".to_string(),
        lines: None,
    };
    assert_eq!(
        resolve_file_ctx(Some(&file), Some(&repo)).as_deref(),
        Some("artifacts/notes.md")
    );
    assert_eq!(resolve_file_ctx(None, None), None);
}

fn headers_with(pairs: &[(&str, &str)]) -> axum::http::HeaderMap {
    let mut h = axum::http::HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            axum::http::HeaderValue::from_str(v).unwrap(),
        );
    }
    h
}

#[test]
fn the_body_device_id_wins_and_a_blank_one_falls_through_to_the_header() {
    let h = headers_with(&[(
        crate::core::subprocess_origin::HEADER_DEVICE_ID,
        "dev-header",
    )]);
    let mut req = base_req(ActorMode::Human);
    assert_eq!(
        crate::api::actor::claimed_device_id(req.device_id.as_deref(), &h),
        Some("dev-header")
    );
    req.device_id = Some("  ".into());
    assert_eq!(
        crate::api::actor::claimed_device_id(req.device_id.as_deref(), &h),
        Some("dev-header")
    );
    req.device_id = Some("dev-body".into());
    assert_eq!(
        crate::api::actor::claimed_device_id(req.device_id.as_deref(), &h),
        Some("dev-body")
    );
    assert_eq!(
        crate::api::actor::claimed_device_id(None, &headers_with(&[])),
        None
    );
}

#[test]
fn a_team_send_takes_the_authenticated_device_over_the_body() {
    use crate::api::actor::authenticated_device_id;
    let h = crate::api::gateway_stamp::test_support::stamped_device_headers("kim-phone");
    assert_eq!(
        authenticated_device_id(Some("sam-phone"), &h),
        Some("kim-phone")
    );
    let none = headers_with(&[]);
    assert_eq!(
        authenticated_device_id(Some("sam-phone"), &none),
        Some("sam-phone")
    );
    assert_eq!(authenticated_device_id(Some(" "), &none), None);
}

#[tokio::test]
async fn a_stale_body_id_beside_a_stamped_header_is_the_stamped_device() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "old-window", None, None, None)
        .await
        .unwrap();
    let h = crate::api::gateway_stamp::test_support::stamped_device_headers("new-window");
    let mut req = base_req(ActorMode::Human);
    req.device_id = Some("old-window".into());

    assert_eq!(sending_device_id(&req, &h), Some("new-window"));
    require_human_mode_is_attributed(&pool, &h, &req)
        .await
        .expect("a stamped id with no row yet still attributes the send");
    let origin = build_message_origin(
        &h,
        req.mode,
        sending_device_id(&req, &h),
        None,
        None,
        None,
        None,
    );
    assert!(
        matches!(&origin, Some(MessageOrigin::Device { device_id }) if device_id == "new-window"),
        "got {origin:?}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db).await;
}

#[test]
fn a_header_only_device_behind_the_gateway_is_recorded_as_that_device() {
    let token = crate::api::local_auth::publish_test_local_token();
    let mut h = headers_with(&[(lucidos_local_token::HEADER_LOCAL_TOKEN, token)]);
    crate::api::gateway_stamp::test_support::insert_stamped_device(&mut h, "dev-1");
    let req = base_req(ActorMode::Human);
    let origin = build_message_origin(
        &h,
        req.mode,
        crate::api::actor::claimed_device_id(req.device_id.as_deref(), &h),
        None,
        None,
        None,
        None,
    );
    assert!(
        matches!(&origin, Some(MessageOrigin::Device { device_id }) if device_id == "dev-1"),
        "got {origin:?}"
    );
    assert!(origin_agrees_with_mode(origin.as_ref(), req.mode).is_ok());
}

#[test]
fn a_mode_the_credential_contradicts_is_a_400_not_a_panic() {
    let agent = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: Some(Uuid::new_v4()),
        emitting_trigger_id: None,
    };
    let refusal = origin_agrees_with_mode(Some(&agent), ActorMode::Engine)
        .expect_err("an agent's credential cannot claim engine mode");
    assert_eq!(refusal.status, StatusCode::BAD_REQUEST);
    assert!(
        refusal.message.contains("\"agent\""),
        "the refusal names the wire value to send back: {}",
        refusal.message
    );
    assert!(origin_agrees_with_mode(Some(&agent), ActorMode::Agent).is_ok());
    assert!(origin_agrees_with_mode(None, ActorMode::Human).is_ok());
}

#[test]
fn caller_workspace_skips_the_subprocess_gate_only_for_a_create() {
    for mode in [ActorMode::Agent, ActorMode::Engine] {
        assert!(
            !subprocess_gate_applies(true, false, mode),
            "an agent's create keeps the spawn-thread --relation top path"
        );
        assert!(
            subprocess_gate_applies(true, true, mode),
            "an existing target keeps the gate"
        );
    }
    assert!(
        subprocess_gate_applies(true, false, ActorMode::Human),
        "a token holder's create cannot claim the user"
    );
    for exists in [true, false] {
        assert!(subprocess_gate_applies(false, exists, ActorMode::Agent));
    }
}

#[test]
fn a_token_holder_naming_another_workspace_cannot_post_into_an_existing_thread() {
    let source = Some(Uuid::new_v4());
    let other = Some(Uuid::new_v4());
    for mode in [ActorMode::Human, ActorMode::Agent, ActorMode::Engine] {
        assert!(subprocess_gate_applies(true, true, mode));
        assert!(!subprocess_chat_legitimate(mode, source, other, None, true));
    }
    assert!(subprocess_gate_applies(true, false, ActorMode::Human));
    assert!(!subprocess_chat_legitimate(
        ActorMode::Human,
        source,
        Some(Uuid::new_v4()),
        None,
        false
    ));
}

#[tokio::test]
async fn an_integrated_spawn_in_human_mode_is_refused_by_name() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let refusal =
        refuse_unfit_integrated_spawn(&pool, Some(Uuid::new_v4()), true, false, None, false, true)
            .await
            .expect_err("human mode cannot carry integrated");
    assert_eq!(refusal.status, StatusCode::BAD_REQUEST);
    assert!(
        refusal.message.contains("--mode human"),
        "{}",
        refusal.message
    );
    crate::test_support::teardown_test_db(&db_name).await;
}

#[test]
fn ordinary_wire_intent_normalizes_aliases_and_retains_context_and_selections() {
    let thread = Uuid::new_v4();
    let event = Uuid::new_v4();
    let raw = serde_json::json!({
        "message": "original", "mode":"human", "thread_id":thread,
        "message_id":event, "use_claude_code":true,
        "image_hashes":["second","first","second"],
        "file_context":{"path":"artifacts/example.md", "lines":[2,5]},
        "model":null, "mentions":["person-example"], "agent_mentioned":true
    });
    let request: ChatRequest = serde_json::from_value(raw).unwrap();
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(wire["event_id"], event.to_string());
    assert_eq!(wire["use_coding_agent"], true);
    assert_eq!(
        wire["image_hashes"],
        serde_json::json!(["second", "first", "second"])
    );
    assert_eq!(wire["file_context"]["lines"], serde_json::json!([2, 5]));
    assert!(wire["model"].is_null());
    assert_eq!(wire["mentions"], serde_json::json!(["person-example"]));
    assert_eq!(wire["agent_mentioned"], true);
    let canonical: ChatRequest = serde_json::from_value(wire).unwrap();
    assert_eq!(
        crate::engine::event_bus::OrdinarySubmissionIntent::new(thread, event, &request).unwrap(),
        crate::engine::event_bus::OrdinarySubmissionIntent::new(thread, event, &canonical).unwrap()
    );
}

#[test]
fn a_coding_agent_orphan_re_runs_as_the_input_it_was() {
    use crate::engine::InjectedPromptKind;
    let id = Uuid::new_v4();
    let starter = |kind| coding_agent_orphan_starter(&kind).map(|starter| starter(id));
    assert_eq!(
        starter(InjectedPromptKind::UserText),
        Some(PreEmittedOrigin::Message(id))
    );
    assert_eq!(
        starter(InjectedPromptKind::ReentryFromEngine),
        Some(PreEmittedOrigin::EngineReentry(id))
    );
    assert_eq!(
        starter(InjectedPromptKind::ReentryFromWait),
        Some(PreEmittedOrigin::WaitReentry(id))
    );
    assert_eq!(starter(InjectedPromptKind::SpokenAside), None);
    assert_eq!(starter(InjectedPromptKind::SpokenContinuation), None);
}

fn human_orphan(text: &str, message_id: Uuid) -> InjectedPrompt {
    InjectedPrompt {
        text: text.into(),
        event_id: Some(message_id),
        mode: ActorMode::Human,
        spawning_event_id: None,
        images: None,
        origin: None,
        kind: crate::engine::InjectedPromptKind::UserText,
    }
}

async fn count_thread_events(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE aggregate_id = $1 AND event_type = $2")
        .bind(thread_id.to_string())
        .bind(event_type)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[test]
fn a_coding_agent_threads_orphan_reaches_its_coding_agent() {
    crate::test_support::run_on_engine_runtime(
        "api::chat::tests::a_coding_agent_threads_orphan_reaches_its_coding_agent",
        coding_agent_orphan_case,
    );
}

async fn coding_agent_orphan_case() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
    let thread_id = crate::test_support::seed_coding_thread(&engine.event_bus, None).await;
    let message_id = engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::test_support::message_received("carry on"),
            meta: crate::engine::thread_events::EventMeta {
                channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                ..crate::engine::thread_events::EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap()
        .event_id;
    let second_id = engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::test_support::message_received("then check"),
            meta: crate::engine::thread_events::EventMeta {
                channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                ..crate::engine::thread_events::EventMeta::NONE
            },
        })
        .await
        .unwrap()
        .unwrap()
        .event_id;
    let (session, mut msg_rx) = crate::engine::AgentSession::for_test();
    engine
        .agent_sessions
        .lock()
        .await
        .insert(thread_id, session);

    process_orphan_chain(
        engine.clone(),
        thread_id,
        vec![
            human_orphan("carry on", message_id),
            human_orphan("then check", second_id),
        ],
    )
    .await;

    let input = msg_rx
        .try_recv()
        .expect("the coding agent's session receives the follow-up");
    assert_eq!(input.origin_event_id, Some(message_id));
    assert!(
        input.kind.writes_prompt_sent(),
        "the run names the message in a CodingAgentPromptSent, got {:?}",
        input.kind
    );
    let second = msg_rx
        .try_recv()
        .expect("the rest of the batch queues in the same run");
    assert_eq!(second.text, "then check");
    assert_eq!(second.origin_event_id, Some(second_id));
    assert!(second.kind.writes_prompt_sent());
    assert!(msg_rx.try_recv().is_err(), "each input queues once");
    let sessions = engine.agent_sessions.lock().await;
    let session = sessions.get(&thread_id).unwrap();
    assert!(session.unforwarded_inputs.contains(&message_id));
    assert!(session.unforwarded_inputs.contains(&second_id));
    drop(sessions);
    for chat_event in [
        "ResponseGenerated",
        "ResponseFailed",
        "ContextCaptured",
        "PromptInjected",
    ] {
        assert_eq!(
            count_thread_events(&pool, thread_id, chat_event).await,
            0,
            "no Lucidos Agent turn runs on a coding-agent thread: found {chat_event}"
        );
    }
    crate::test_support::teardown_test_db(&db).await;
}

#[test]
fn an_unknown_lane_runs_no_turn_and_says_so() {
    crate::test_support::run_on_engine_runtime(
        "api::chat::tests::an_unknown_lane_runs_no_turn_and_says_so",
        unknown_lane_case,
    );
}

async fn unknown_lane_case() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
    let thread_id = Uuid::new_v4();

    process_orphan_chain(
        engine.clone(),
        thread_id,
        vec![human_orphan("carry on", Uuid::new_v4())],
    )
    .await;

    for chat_event in ["ResponseGenerated", "ContextCaptured", "PromptInjected"] {
        assert_eq!(
            count_thread_events(&pool, thread_id, chat_event).await,
            0,
            "an unknown lane runs no Lucidos Agent turn: found {chat_event}"
        );
    }
    assert_eq!(
        count_thread_events(&pool, thread_id, "ResponseFailed").await,
        1,
        "the refusal is visible on the thread"
    );
    crate::test_support::teardown_test_db(&db).await;
}

#[test]
fn a_caller_without_a_lane_reaches_the_threads_coding_agent() {
    crate::test_support::run_on_engine_runtime(
        "api::chat::tests::a_caller_without_a_lane_reaches_the_threads_coding_agent",
        caller_without_a_lane_case,
    );
}

async fn caller_without_a_lane_case() {
    let (pool, db) = crate::test_support::setup_test_db().await;
    let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
    let thread_id = crate::test_support::seed_coding_thread(&engine.event_bus, None).await;
    let (session, mut msg_rx) = crate::engine::AgentSession::for_test();
    engine
        .agent_sessions
        .lock()
        .await
        .insert(thread_id, session);

    engine
        .process_message_with_steps(
            "carry on",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(thread_id),
            None,
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            None,
            None,
            None,
            crate::engine::FollowUpUrgency::Normal,
            None,
        )
        .await
        .unwrap();

    let input = msg_rx
        .try_recv()
        .expect("the coding agent's session receives the message");
    assert_eq!(input.text, "carry on");
    for chat_event in ["ResponseGenerated", "ResponseFailed", "ContextCaptured"] {
        assert_eq!(
            count_thread_events(&pool, thread_id, chat_event).await,
            0,
            "no Lucidos Agent turn runs on a coding-agent thread: found {chat_event}"
        );
    }
    crate::test_support::teardown_test_db(&db).await;
}
