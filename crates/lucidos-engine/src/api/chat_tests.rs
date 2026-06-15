use super::*;

// ── subprocess_chat_legitimate ──────────────────────────────────────
//
// Regression coverage for the cross-thread incident: an LLM agent in
// thread A used `curl POST /api/v1/changes/<id>/apply` to act on thread B,
// and the resulting event was misattributed as a "You" card. These
// tests pin the chat-POST allow/deny matrix that prevents the
// equivalent attack on `/api/v1/chat/stream` (where the impact would be
// worse — kicking thread B's agentic loop under a false user identity).

#[test]
fn subprocess_chat_rejects_human_mode_targeting_any_thread() {
    let source = Some(Uuid::new_v4());
    // Cross-thread human mode (the actual attack shape) — reject.
    let target_other = Some(Uuid::new_v4());
    assert!(!subprocess_chat_legitimate(
        ActorMode::Human,
        source,
        target_other,
        None
    ));
    // Same-thread human mode — also reject. A subprocess never *is*
    // the user, even when targeting its own thread.
    assert!(!subprocess_chat_legitimate(
        ActorMode::Human,
        source,
        source,
        None
    ));
    // No target / no parent — reject (nothing to legitimise).
    assert!(!subprocess_chat_legitimate(
        ActorMode::Human,
        source,
        None,
        None,
    ));
}

#[test]
fn subprocess_chat_rejects_cross_thread_agent_post() {
    // Agent mode targeting a thread the subprocess didn't spawn from
    // and no parent claim — the canonical cross-thread injection.
    let source = Some(Uuid::new_v4());
    let target_other = Some(Uuid::new_v4());
    assert!(!subprocess_chat_legitimate(
        ActorMode::Agent,
        source,
        target_other,
        None,
    ));
    assert!(!subprocess_chat_legitimate(
        ActorMode::Engine,
        source,
        target_other,
        None,
    ));
}

#[test]
fn subprocess_chat_allows_same_thread_agent_followup() {
    let source = Some(Uuid::new_v4());
    // `target_thread_id == source_thread_id` — an agent posting back
    // into its own loop (the channel `lucidos spawn-thread` uses on
    // follow-ups within the same thread).
    assert!(subprocess_chat_legitimate(
        ActorMode::Agent,
        source,
        source,
        None,
    ));
    assert!(subprocess_chat_legitimate(
        ActorMode::Engine,
        source,
        source,
        None,
    ));
}

#[test]
fn subprocess_chat_allows_spawn_with_matching_parent_thread() {
    // The `lucidos spawn-thread` shape: target_thread_id = None
    // (engine generates the new sub-thread), parent_thread_id =
    // source — exactly the spawn graph the engine wants.
    let source = Some(Uuid::new_v4());
    assert!(subprocess_chat_legitimate(
        ActorMode::Agent,
        source,
        None,
        source,
    ));
}

#[test]
fn subprocess_chat_rejects_spawn_with_mismatched_parent_thread() {
    // Agent mode claiming a parent_thread_id that doesn't match the
    // subprocess's actual source — denied. A subprocess of thread A
    // can't pretend to be spawning from thread C.
    let source = Some(Uuid::new_v4());
    let lied_parent = Some(Uuid::new_v4());
    assert!(!subprocess_chat_legitimate(
        ActorMode::Agent,
        source,
        None,
        lied_parent,
    ));
}

#[test]
fn subprocess_chat_no_source_thread_still_rejects_cross_thread() {
    // Subprocess presented the token but no source-thread-id header
    // (older subprocess, raw curl that forgot the second header).
    // We can't link to a source, so any agent-mode post that isn't a
    // follow-up to *some* thread we can name is rejected.
    let target_other = Some(Uuid::new_v4());
    assert!(!subprocess_chat_legitimate(
        ActorMode::Agent,
        None,
        target_other,
        None,
    ));
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
        images: None,
        image_hashes: None,
        device_id: None,
        use_claude_code: None,
        coding_agent: None,
        cc_model: None,
        event_id: None,
        thread_id: None,
        conflict_change_id: None,
        repo_id: None,
        folder: None,
        title: None,
        mode,
        parent_thread_id: None,
        spawning_event_id: None,
        caller_workspace: None,
        caller_thread_id: None,
        caller_event_id: None,
    }
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
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn human_mode_with_spawning_event_id_returns_400() {
    let mut req = base_req(ActorMode::Human);
    req.spawning_event_id = Some(Uuid::new_v4().to_string());
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
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
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn engine_mode_without_parent_thread_id_returns_400() {
    let req = base_req(ActorMode::Engine);
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn engine_mode_with_caller_workspace_is_valid() {
    let mut req = base_req(ActorMode::Engine);
    req.caller_workspace = Some("personal".into());
    assert!(validate_mode_and_spawn(&req).is_ok());
}

#[test]
fn invalid_parent_uuid_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.parent_thread_id = Some("not-a-uuid".into());
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn invalid_spawning_event_uuid_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.parent_thread_id = Some(Uuid::new_v4().to_string());
    req.spawning_event_id = Some("not-a-uuid".into());
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn chat_request_requires_mode() {
    let body = serde_json::json!({
        "message": "spawned task",
        "use_claude_code": true,
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
fn caller_workspace_with_parent_thread_id_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("personal".into());
    req.parent_thread_id = Some(Uuid::new_v4().to_string());
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn caller_workspace_with_spawning_event_id_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("personal".into());
    req.spawning_event_id = Some(Uuid::new_v4().to_string());
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn caller_thread_id_without_caller_workspace_returns_400() {
    // caller_thread_id only meaningful in conjunction with caller_workspace —
    // an orphan caller id is a malformed request, not a silent drop.
    let mut req = base_req(ActorMode::Human);
    req.caller_thread_id = Some(Uuid::new_v4().to_string());
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn caller_event_id_without_caller_workspace_returns_400() {
    let mut req = base_req(ActorMode::Human);
    req.caller_event_id = Some(Uuid::new_v4().to_string());
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn caller_thread_id_invalid_uuid_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("personal".into());
    req.caller_thread_id = Some("not-a-uuid".into());
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn caller_event_id_invalid_uuid_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("personal".into());
    req.caller_event_id = Some("not-a-uuid".into());
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn caller_workspace_with_only_workspace_field_is_valid() {
    // Caller workspace alone is fine — caller_thread_id / caller_event_id
    // are optional. Common case: human curl from another workspace.
    let mut req = base_req(ActorMode::Human);
    req.caller_workspace = Some("personal".into());
    let v = validate_mode_and_spawn(&req).unwrap();
    assert_eq!(v.parent_thread_id, None);
    assert_eq!(v.spawning_event_id, None);
}

#[test]
fn caller_workspace_with_all_three_fields_is_valid() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("personal".into());
    req.caller_thread_id = Some(Uuid::new_v4().to_string());
    req.caller_event_id = Some(Uuid::new_v4().to_string());
    assert!(validate_mode_and_spawn(&req).is_ok());
}

// -- validate_thread_continuity ---------------------------------------

#[test]
fn continuity_new_thread_is_always_ok() {
    // No existing summary => new thread, anything goes
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
        validate_thread_continuity(Some("chat"), None, None, Some(true), Some("repo-a"), None).unwrap_err();
    assert_eq!(err.0, StatusCode::CONFLICT);
    assert!(err.1.contains("Lucidos"));
    assert!(err.1.contains("Claude Code"));
}

#[test]
fn continuity_cc_thread_rejects_chat_followup() {
    let err = validate_thread_continuity(Some("claude_code"), Some("repo-a"), None, None, None, None)
        .unwrap_err();
    assert_eq!(err.0, StatusCode::CONFLICT);
    let err =
        validate_thread_continuity(Some("claude_code"), Some("repo-a"), None, Some(false), None, None)
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
    // Request omits repo_id => frontend will inherit from the thread.
    // Don't 409 just because the field is missing.
    assert!(
        validate_thread_continuity(Some("claude_code"), Some("repo-a"), None, Some(true), None, None)
            .is_ok()
    );
}

#[test]
fn continuity_cc_thread_with_no_existing_repo_is_ok() {
    // First Claude Code session bound but cc_repo_id wasn't recorded (e.g. older
    // event before SessionStarted carried repo_id). Don't gate on a
    // missing existing value — just let the request through.
    assert!(
        validate_thread_continuity(Some("claude_code"), None, None, Some(true), Some("repo-b"), None)
            .is_ok()
    );
}

#[test]
fn continuity_cc_thread_rejects_backend_flip() {
    // Thread locked to claude-code (explicit) — requesting codex must 409.
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

    // Legacy row (NULL stored backend) is a claude-code thread — same 409.
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
    // Omitted request backend always passes — the engine resolves from the
    // stored value.
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
    // Trigger threads aren't claude_code, so use_claude_code=true is a
    // mode switch and must be rejected.
    let err = validate_thread_continuity(Some("trigger"), None, None, Some(true), None, None).unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
    }

    // ── drain_orphan_queue ──

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

    #[tokio::test]
    async fn drain_orphan_queue_processes_each_initial_orphan_in_order() {
        let initial = vec![make_orphan("a"), make_orphan("b"), make_orphan("c")];
    let processed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let processed_clone = processed.clone();
    drain_orphan_queue(initial, move |orphan| {
        let processed = processed_clone.clone();
        async move {
            processed.lock().unwrap().push(orphan.text.clone());
            Vec::new()
        }
    })
    .await;
    assert_eq!(*processed.lock().unwrap(), vec!["a", "b", "c"]);
}

// Regression: a re-processed orphan whose own loop produces NEW orphans
// (the in-the-wild thread 9b5a05aa scenario where the user sent two
// follow-ups in quick succession during recovery) used to lose those
// child orphans because the spawned task discarded the ProcessResult.
// The chain must keep draining until every appended orphan is processed.
#[tokio::test]
async fn drain_orphan_queue_processes_orphans_of_orphans() {
    // Orphan "a" produces orphan "b" when processed; orphan "b" produces
    // nothing. Both must be processed.
    let initial = vec![make_orphan("a")];
    let processed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let processed_clone = processed.clone();
    drain_orphan_queue(initial, move |orphan| {
        let processed = processed_clone.clone();
        async move {
            let text = orphan.text.clone();
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
    // Each orphan produces one more orphan, four levels deep.
    let initial = vec![make_orphan("0")];
    let processed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let processed_clone = processed.clone();
    drain_orphan_queue(initial, move |orphan| {
        let processed = processed_clone.clone();
        async move {
            let n: i32 = orphan.text.parse().unwrap();
            processed.lock().unwrap().push(orphan.text);
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
    drain_orphan_queue(vec![], move |_orphan| {
        let processed = processed_clone.clone();
        async move {
            *processed.lock().unwrap() += 1;
            Vec::new()
        }
    })
    .await;
    assert_eq!(*processed.lock().unwrap(), 0);
}
