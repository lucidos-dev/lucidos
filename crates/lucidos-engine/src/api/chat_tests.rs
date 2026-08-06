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
        None,
        true,
    ));
    // Same-thread human mode — also reject. A subprocess never *is*
    // the user, even when targeting its own thread.
    assert!(!subprocess_chat_legitimate(
        ActorMode::Human,
        source,
        source,
        None,
        true,
    ));
    // No target / no parent — reject (nothing to legitimise).
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
    // Agent mode targeting a thread the subprocess didn't spawn from
    // and no parent claim — the canonical cross-thread injection.
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
    // `target_thread_id == source_thread_id` — an agent posting back
    // into its own loop (the channel `lucidos spawn-thread` uses on
    // follow-ups within the same thread).
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
    // The `lucidos spawn-thread` shape: target_thread_id = None
    // (engine generates the new sub-thread), parent_thread_id =
    // source — exactly the spawn graph the engine wants.
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
        false,
    ));
}

/// Hazard 10(b) of the child-follow-up plan. `parent_matches_source` says
/// "I am spawning a child"; nothing in that claim constrains the TARGET.
/// Without the `!target_exists` conjunct, a subprocess of thread S could
/// write into any existing thread by naming it as the target and naming
/// itself as the parent, reaching the same cross-thread injection the
/// `target_matches_source` arm refuses through the other arm.
///
/// After the origin token became thread-bound this was the last route by
/// which an authenticated subprocess could write into a thread it does not
/// own, and it would have made `POST /threads/:thread_id/follow-up`'s
/// refusal ladder bypassable.
#[test]
fn parent_matches_source_requires_a_new_target() {
    let source = Some(Uuid::new_v4());
    let someone_elses_thread = Some(Uuid::new_v4());

    for mode in [ActorMode::Agent, ActorMode::Engine] {
        assert!(
            !subprocess_chat_legitimate(mode, source, someone_elses_thread, source, true),
            "{mode:?}: claiming parenthood must not open an EXISTING thread"
        );
        // Same call, target does not exist yet: that is a spawn, and allowed.
        assert!(
            subprocess_chat_legitimate(mode, source, someone_elses_thread, source, false),
            "{mode:?}: a spawn naming its own new thread id stays allowed"
        );
    }
}

/// The regression the deferral asked for. `lucidos spawn-thread --relation
/// child` pre-generates a client-side uuid and sends it together with
/// `parent_thread_id`, so it hits the spawn arm with a target that provably
/// does not exist. See `crates/lucidos-cli/src/spawn_thread.rs`.
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

/// The tightening did not overreach: a subprocess following up on its OWN
/// thread is allowed precisely because that thread exists, and it reaches
/// the `target_matches_source` arm, which the new conjunct does not touch.
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
    // And still allowed with the parent claim alongside it, which is what a
    // subprocess re-sending into its own thread with spawn context looks like.
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
    // A thread-less subprocess: a scheduled script, whose origin token is
    // minted against the no-thread sentinel. There is no source thread to
    // compare against, so neither arm can match and any agent-mode post is
    // rejected.
    let target_other = Some(Uuid::new_v4());
    assert!(!subprocess_chat_legitimate(
        ActorMode::Agent,
        None,
        target_other,
        None,
        true,
    ));
}

// ── human_mode_is_attributed ────────────────────────────────────────
//
// Regression coverage for the 2026-08-06 incident: the Lucidos Agent shelled
// out to `curl` and POSTed `mode: "human"` to /api/v1/chat/stream, and the
// engine recorded it as a turn the user typed. The matrix above never saw the
// request, because it only runs for a caller that PRESENTS an origin token,
// and curl does not forward one. These tests pin the gate that runs for every
// caller.

#[test]
fn unattributed_human_mode_is_refused() {
    // The incident shape exactly: no device, no caller workspace.
    assert!(!human_mode_is_attributed(ActorMode::Human, false, false));
}

#[test]
fn a_registered_device_attributes_a_human() {
    // The user's own client: it sends `x-lucidos-device-id` on every mutating
    // fetch, and the id resolves to a `devices` row.
    assert!(human_mode_is_attributed(ActorMode::Human, true, false));
}

#[test]
fn a_cross_workspace_caller_may_still_speak_for_its_human() {
    // The existing cross-workspace contract: the calling workspace vouches for
    // its own user. Still only a display hint (`api::actor`), and deliberately
    // not renegotiated by this gate.
    assert!(human_mode_is_attributed(ActorMode::Human, false, true));
}

#[test]
fn agent_and_engine_modes_need_no_human_evidence() {
    // They claim no human, so there is nothing to substantiate here.
    // `validate_mode_and_spawn` requires their provenance and
    // `subprocess_chat_legitimate` constrains their reach.
    for mode in [ActorMode::Agent, ActorMode::Engine] {
        assert!(
            human_mode_is_attributed(mode, false, false),
            "mode: {mode:?}"
        );
    }
}

/// The root cause, pinned directly: **dropping the origin token must never buy
/// more than presenting it.**
///
/// Before this gate, a subprocess that presented its token was held to
/// `subprocess_chat_legitimate` (which refuses `mode: Human` outright), while
/// the same subprocess shelling out to `curl` read as an ordinary external API
/// client and was allowed. The constraint was opt-in by the party it
/// constrained. For every unattributed body, the token-absent path must now be
/// no more permissive than the token-present one.
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
    // The incident's real failure was the agent not reporting that it was
    // blocked, so the message has to carry the alternative, not just the "no".
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

// ── thread_target_is_addressable ────────────────────────────────────
//
// The other half of the incident: the POST hit the wrong engine, whose
// `MessageReceived` projection is an upsert, so the six unknown thread ids were
// materialized there instead of refused. Reading them back off the same wrong
// engine then FOUND them, so the agent's own verification confirmed the
// mistake.

#[test]
fn an_existing_thread_is_always_addressable() {
    assert!(thread_target_is_addressable(true, None, None, false));
}

#[test]
fn an_unknown_thread_with_no_create_signal_is_refused() {
    assert!(!thread_target_is_addressable(false, None, None, false));
    // An explicit `false` is not a create signal either.
    assert!(!thread_target_is_addressable(
        false,
        Some(false),
        None,
        false
    ));
}

#[test]
fn each_create_signal_admits_an_unknown_thread() {
    // The frontend's raw new send, which mints its own uuid.
    assert!(thread_target_is_addressable(false, Some(true), None, false));
    // A same-workspace spawn with callback (`lucidos spawn-thread --relation
    // child`), which is why that CLI needed no new flag.
    assert!(thread_target_is_addressable(
        false,
        None,
        Some(Uuid::new_v4()),
        false
    ));
    // A cross-workspace spawn (`workspace_client`, `spawn-thread --to`), same.
    assert!(thread_target_is_addressable(false, None, None, true));
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
        use_coding_agent: None,
        coding_agent: None,
        cc_model: None,
        event_id: None,
        thread_id: None,
        new_thread: None,
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
    req.caller_workspace = Some("dev".into());
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
    // Back-compat: payloads persisted / sent before the
    // `use_claude_code` → `use_coding_agent` rename (queued ThreadQueueRequest
    // rows, in-flight clients) must still deserialize via the serde alias.
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
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn caller_workspace_with_spawning_event_id_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("dev".into());
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
    req.caller_workspace = Some("dev".into());
    req.caller_thread_id = Some("not-a-uuid".into());
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn caller_event_id_invalid_uuid_returns_400() {
    let mut req = base_req(ActorMode::Agent);
    req.caller_workspace = Some("dev".into());
    req.caller_event_id = Some("not-a-uuid".into());
    assert_eq!(validate_mode_and_spawn(&req), Err(StatusCode::BAD_REQUEST));
}

#[test]
fn caller_workspace_with_only_workspace_field_is_valid() {
    // Caller workspace alone is fine — caller_thread_id / caller_event_id
    // are optional. Common case: human curl from another workspace.
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
    // Request omits repo_id => frontend will inherit from the thread.
    // Don't 409 just because the field is missing.
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
    // First Claude Code session bound but cc_repo_id wasn't recorded (e.g. older
    // event before SessionStarted carried repo_id). Don't gate on a
    // missing existing value — just let the request through.
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
    // Trigger threads aren't coding-agent threads, so use_coding_agent=true is
    // a mode switch and must be rejected.
    let err = validate_thread_continuity(Some("trigger"), None, None, Some(true), None, None)
        .unwrap_err();
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

fn make_wake(text: &str) -> InjectedPrompt {
    InjectedPrompt {
        text: text.to_string(),
        event_id: Some(Uuid::new_v4()),
        mode: ActorMode::Agent,
        spawning_event_id: Some(Uuid::new_v4()),
        images: None,
        origin: None,
        kind: crate::engine::InjectedPromptKind::WakeFromChild,
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
async fn drain_orphan_queue_keeps_wake_from_child_separate() {
    let initial = vec![make_orphan("a"), make_wake("wake"), make_orphan("b")];
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
            vec!["wake".to_string()],
            vec!["b".to_string()]
        ]
    );
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
    // Each orphan produces one more orphan, four levels deep.
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
async fn queued_message_lookup_binds_thread_aggregate_id_as_text() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _cb_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let injected_message_id = Uuid::new_v4();
    let removed_message_id = Uuid::new_v4();

    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "queued".into(),
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
            event_id: Some(injected_message_id),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("MessageReceived must persist");

    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserPromptInjected {
            text: "queued".into(),
            mode: ActorMode::Human,
            origin: None,
            injected_message_id: Some(injected_message_id),
            delivered_event_id: None,
        },
        meta: EventMeta {
            request_event_id: Some(injected_message_id),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("UserPromptInjected must persist");

    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "remove me".into(),
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
            event_id: Some(removed_message_id),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("second MessageReceived must persist");

    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::QueuedMessageRemoved { removed_message_id },
        meta: EventMeta::NONE,
    })
    .await
    .expect("QueuedMessageRemoved must persist");

    assert!(
        queued_message_already_injected(&pool, thread_id, injected_message_id)
            .await
            .expect("injected lookup must not type-error"),
        "injected lookup must find the matching marker"
    );
    assert!(
        queued_message_already_removed(&pool, thread_id, removed_message_id)
            .await
            .expect("removed lookup must not type-error"),
        "removed lookup must find the matching marker"
    );
    assert!(
        !queued_message_already_injected(&pool, thread_id, removed_message_id)
            .await
            .expect("non-match lookup must not type-error"),
        "lookup must not match the wrong marker field"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

// ── announce_orphan_batch ──

/// Persist a `MessageReceived` under a caller-chosen event id, so the test can
/// build an orphan that names it via `injected_message_id`.
async fn persist_message(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    text: &str,
    event_id: Uuid,
) {
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: text.into(),
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

/// The shape the user hit: message A starts a turn, message B lands while it is
/// running (so it is injected, not queued behind a fresh turn), then the user's
/// Stop terminates A. B is recovered as an orphan and re-submitted as a turn of
/// its own, anchored on the `MessageReceived` that already exists for it.
///
/// Nothing else in that re-processed turn sets the thread back to `running`, so
/// the batch announcement is what keeps the projection honest while the agent
/// works. Before the fix the first orphan was skipped and the thread read
/// `idle` for the whole turn.
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
        &[orphan_naming(msg_b, "actually, just the totals")],
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
         WHERE aggregate_id = $1 AND event_type = 'UserPromptInjected' ORDER BY sequence",
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

/// Every orphan in the batch is announced, not just the ones after the first.
/// The batch is coalesced into ONE re-processed turn, so all of them anchor on
/// the first message's request id while naming their own.
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

    announce_orphan_batch(&bus, thread_id, &batch).await;

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT payload->>'injected_message_id', payload->>'request_event_id' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'UserPromptInjected' ORDER BY sequence",
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

/// An empty batch cannot happen through `process_orphan_chain` (the caller has
/// already read `first`), but the helper must not panic if one ever reaches it.
#[tokio::test]
async fn announce_orphan_batch_is_a_noop_for_an_empty_batch() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _cb_rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    announce_orphan_batch(&bus, thread_id, &[]).await;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'UserPromptInjected'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("query");
    assert_eq!(count, 0);

    crate::test_support::teardown_test_db(&db_name).await;
}

// ── resolve_file_ctx ────────────────────────────────────────────────
//
// Both file previews render the same line-numbered source view, so a workspace
// data file carries a selected line range exactly as a repository file does.

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
