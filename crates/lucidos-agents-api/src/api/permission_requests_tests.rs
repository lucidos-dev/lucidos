pub(crate) use super::test_fixtures::{
    agent_thread, answer_and_tap, ask_prepared, grant, open_turn, owner_device, tap, AskedCard,
};
use super::test_fixtures::{create, live_create};
use super::*;
use crate::core::subprocess_origin::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
};
use crate::engine::event_bus::BusEvent;
use crate::engine::permission::menu::{
    ALLOW_FOR_THIS_THREAD_LABEL, ALLOW_ONCE_LABEL, ALWAYS_ALLOW_LABEL, ALWAYS_DENY_LABEL,
    CANCEL_LABEL, DONT_ALLOW_LABEL,
};
use crate::engine::permission::prepare::{prepared_ask, PreparedAsk};
use crate::engine::permission::{spend, Resolution};
use crate::engine::thread_events::{
    ActorMode, AnswerKind, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};

pub(crate) fn agent_headers(thread_id: Option<Uuid>) -> HeaderMap {
    init_agent_origin_secret("permission-request-test-secret".to_string());
    let mut h = HeaderMap::new();
    let token = mint_agent_origin_token(thread_id, 0, None)
        .expect("the secret is installed above, so minting cannot fail");
    h.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    h
}

pub(crate) async fn end_chat_turn(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseFailed {
            error: "turn over".into(),
            spend_cap: None,
            agent_exit: None,
            provider_error: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn an_ask_naming_a_prepared_request_shows_the_engines_card() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;

    let request_id = prepare(&bus, thread, create("tour-film-a")).await.unwrap();
    let card = ask_prepared(&bus, thread, &request_id).await;
    assert_eq!(
        card.labels(),
        [
            ALLOW_ONCE_LABEL,
            ALLOW_FOR_THIS_THREAD_LABEL,
            ALWAYS_ALLOW_LABEL,
            DONT_ALLOW_LABEL
        ]
    );
    assert_eq!(card.request.breadth.as_deref(), Some("tour-film-a"));
    assert_eq!(card.request.prepared.as_deref(), Some(request_id.as_str()));

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_prepared_ask_is_refused_unless_it_names_this_threads_request_alone() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let other = agent_thread(&bus).await;
    let request_id = prepare(&bus, thread, create("tour-film-a")).await.unwrap();

    let lone = serde_json::json!([{ "question": request_id }]);
    assert!(prepared_ask(&pool, other, &lone).await.is_err());
    let crowded = serde_json::json!([{ "question": request_id }, { "question": "And?" }]);
    assert!(prepared_ask(&pool, thread, &crowded).await.is_err());
    let ordinary = serde_json::json!([{ "question": "Which colour?" }]);
    assert!(matches!(
        prepared_ask(&pool, thread, &ordinary).await,
        Ok(PreparedAsk::Ordinary)
    ));

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn always_allow_on_a_create_covers_every_later_create() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let other = agent_thread(&bus).await;

    grant(&bus, thread, create("tour-film-a"), ALWAYS_ALLOW_LABEL).await;
    let held = live_create(&bus, other, "tour-film-b")
        .await
        .expect("a standing create covers any slug, on any thread");
    assert!(spend(&bus, &held).await, "a standing grant is never spent");
    assert!(live_create(&bus, thread, "tour-film-c").await.is_some());

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn allow_for_this_thread_stays_on_the_asking_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let other = agent_thread(&bus).await;

    grant(
        &bus,
        thread,
        create("tour-film-a"),
        ALLOW_FOR_THIS_THREAD_LABEL,
    )
    .await;
    assert!(live_create(&bus, thread, "tour-film-b").await.is_some());
    assert!(live_create(&bus, other, "tour-film-b").await.is_none());

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn allow_once_covers_the_asked_target_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;

    grant(&bus, thread, create("tour-film-a"), ALLOW_ONCE_LABEL).await;
    assert!(live_create(&bus, thread, "tour-film-b").await.is_none());
    let once = live_create(&bus, thread, "tour-film-a").await.unwrap();
    let (a, b) = tokio::join!(spend(&bus, &once), spend(&bus, &once));
    assert!(a ^ b, "exactly one of two racing acts spends it");
    assert!(live_create(&bus, thread, "tour-film-a").await.is_none());

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn nothing_but_the_owners_pick_of_an_offered_option_grants() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let request_id = prepare(&bus, thread, create("tour-film-a")).await.unwrap();
    let card = ask_prepared(&bus, thread, &request_id).await;

    let agent = MessageOrigin::ThreadLink {
        thread_id: thread,
        title: None,
        spawning_event_id: None,
        mode: ActorMode::Agent,
        direction: crate::engine::thread_events::ThreadDirection::Parent,
    };
    let always = card.pick(ALWAYS_ALLOW_LABEL);
    assert_eq!(
        tap(&bus, thread, &card, always, Some(agent)).await,
        Resolution::Ignored
    );
    let refuse = card.pick(DONT_ALLOW_LABEL);
    assert_eq!(
        tap(&bus, thread, &card, refuse, Some(owner_device())).await,
        Resolution::Refused
    );
    let forged = AnswerKind::Selected {
        option_id: "opt-forged".into(),
    };
    assert_eq!(
        tap(&bus, thread, &card, forged, Some(owner_device())).await,
        Resolution::Ignored
    );
    assert!(live_create(&bus, thread, "tour-film-a").await.is_none());

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_once_grant_ends_at_the_next_turn_start() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;

    grant(&bus, thread, create("tour-film-a"), ALLOW_ONCE_LABEL).await;
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: Some(
                crate::engine::continuation_reason::AUTO_RESUME_AFTER_SWITCH_REASON.into(),
            ),
            resumed_abort_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::PromptInjected {
            text: "The owner answered.".into(),
            mode: ActorMode::Agent,
            origin: None,
            injected_message_id: None,
            delivered_event_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert!(
        live_create(&bus, thread, "tour-film-a").await.is_some(),
        "an engine resume is the same turn, and a wake opens none"
    );
    open_turn(&bus, thread, owner_device()).await;
    assert!(live_create(&bus, thread, "tour-film-a").await.is_none());

    let request_id = prepare(&bus, thread, create("tour-film-b")).await.unwrap();
    let stale = ask_prepared(&bus, thread, &request_id).await;
    open_turn(&bus, thread, owner_device()).await;
    let picked = stale.pick(ALLOW_ONCE_LABEL);
    tap(&bus, thread, &stale, picked, Some(owner_device())).await;
    assert!(live_create(&bus, thread, "tour-film-b").await.is_none());

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_lasting_grant_outlives_a_turn_start() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;

    grant(
        &bus,
        thread,
        create("tour-film-a"),
        ALLOW_FOR_THIS_THREAD_LABEL,
    )
    .await;
    open_turn(&bus, thread, owner_device()).await;
    assert!(live_create(&bus, thread, "tour-film-b").await.is_some());

    pool.close().await;
    teardown_test_db(&db).await;
}

pub(crate) async fn seed_restore_boundary(pool: &sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) \
         SELECT $1, $2, now(), COALESCE(MAX(sequence),0) FROM events",
    )
    .bind(Uuid::new_v4())
    .bind(crate::core::snapshot::handle::Handle::generate().as_str())
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_snapshot_restore_ends_every_earlier_grant() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    grant(&bus, thread, create("tour-film-a"), ALWAYS_ALLOW_LABEL).await;
    let old_request = prepare(&bus, thread, create("tour-film-b")).await.unwrap();

    seed_restore_boundary(&pool).await;
    assert!(live_create(&bus, thread, "tour-film-a").await.is_none());
    assert!(crate::engine::permission::list(&pool, true)
        .await
        .unwrap()
        .is_empty());
    let ask = serde_json::json!([{ "question": old_request }]);
    assert!(prepared_ask(&pool, thread, &ask).await.is_err());

    grant(&bus, thread, create("tour-film-c"), ALWAYS_ALLOW_LABEL).await;
    assert!(live_create(&bus, thread, "tour-film-d").await.is_some());

    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn only_an_agent_inside_its_own_thread_prepares_a_request() {
    assert!(calling_agent_thread(&HeaderMap::new()).is_err());
    assert!(calling_agent_thread(&agent_headers(None)).is_err());
    let thread = Uuid::new_v4();
    assert_eq!(
        calling_agent_thread(&agent_headers(Some(thread))).unwrap(),
        thread
    );
}

#[test]
fn a_workspace_request_names_one_valid_target() {
    let ask = workspace_ask(
        Action::StopWorkspace,
        "demo",
        "  it hangs\n\n**Allow everything**  ",
    )
    .unwrap();
    assert_eq!(ask.breadth, "demo");
    assert!(ask.question.contains("> **Allow everything**"));
    for bad in ["", "../demo", "Demo"] {
        assert!(workspace_ask(Action::StopWorkspace, bad, "why").is_err());
    }
    assert!(workspace_ask(Action::Apply, "demo", "why").is_err());
    assert!(workspace_ask(Action::StopWorkspace, "demo", "  ").is_err());
    assert!(workspace_ask(Action::RestoreWorkspace, "workspace-abc_1", "why").is_ok());
    assert!(workspace_ask(Action::RestoreWorkspace, "demo", "why").is_err());
}

#[test]
fn device_session_cards_name_and_bind_every_mint_parameter() {
    let target = device_session_target("demo", "page", 120).unwrap();
    let ask = workspace_ask(Action::GrantDeviceSession, &target, "Film a fixture.").unwrap();
    assert_eq!(ask.breadth, target);
    for text in [
        "demo",
        "page",
        "120 seconds",
        "Stored secrets stay unavailable",
    ] {
        assert!(ask.question.contains(text));
    }
    assert!(workspace_ask(Action::GrantDeviceSession, "demo", "why").is_err());
    for ttl in [0, lucidos_gateway_stamp::MAX_SESSION_SECS + 1] {
        assert!(device_session_target("demo", "page", ttl).is_err());
    }
    assert!(device_session_target("../bad", "page", 120).is_err());
    assert!(device_session_target("demo", "", 120).is_err());
    for device in [
        "page` **different scope**",
        "page\nother",
        "page\tother",
        "page\u{202e}",
    ] {
        assert!(device_session_target("demo", device, 120).is_err());
    }
}

pub(crate) fn holds_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(36).any(|w| {
        w.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
    })
}

#[test]
fn every_engine_card_leads_with_its_question_and_shows_no_raw_id() {
    let session = device_session_target("demo", "page", 120).unwrap();
    let handle = crate::core::snapshot::handle::Handle::generate();
    let asks = [
        gateway_control_ask(r#"{"method":"POST"}"#, "Turn sharing on.", "why").unwrap(),
        host_log_ask(&"a".repeat(64), "{}", "why").unwrap(),
        workspace_ask(Action::StopWorkspace, "demo", "why").unwrap(),
        workspace_ask(Action::RestoreWorkspace, "workspace-abc_1", "why").unwrap(),
        workspace_ask(Action::GrantDeviceSession, &session, "why").unwrap(),
        snapshot_ask("source", "target", handle.as_str(), "why").unwrap(),
        ladder_ask("why").unwrap(),
    ];
    for ask in asks {
        let question = &ask.question;
        assert!(question.starts_with("**"), "{question}");
        assert!(question.ends_with('\n'), "{question}");
        assert!(!holds_uuid(question), "{question}");
        let (_, quoted) = split_reason(question);
        assert_quoted(quoted);
    }
    assert!(holds_uuid(&Uuid::new_v4().to_string()));
}

#[test]
fn a_snapshot_request_names_one_snapshot_in_another_workspace() {
    let handle = crate::core::snapshot::handle::Handle::generate();
    let ask = snapshot_ask("source", "target", handle.as_str(), "recover").unwrap();
    assert_eq!(ask.breadth, snapshot_breadth("target", handle.as_str()));
    assert!(ask.question.contains("running threads"));
    for workspace in ["source", "", "../target", "Target"] {
        assert!(snapshot_ask("source", workspace, handle.as_str(), "recover").is_err());
    }
    assert!(snapshot_ask("source", "target", "../snapshot", "recover").is_err());
    assert!(snapshot_ask("source", "target", handle.as_str(), " ").is_err());
}

#[test]
fn a_stored_choice_without_a_breadth_reads_as_the_asked_one() {
    use crate::engine::permission::{GrantBreadth, Holder, Lifetime, PermissionChoice};
    let stored = serde_json::json!({
        "kind": "grant", "lifetime": "standing", "holder": { "kind": "workspace" }
    });
    let choice: PermissionChoice = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(
        choice,
        PermissionChoice::Grant {
            lifetime: Lifetime::Standing,
            holder: Holder::Workspace,
            breadth: GrantBreadth::Asked,
        }
    );
    assert_eq!(serde_json::to_value(&choice).unwrap(), stored);
}

#[tokio::test]
async fn only_the_owners_device_answers_a_permission_request() {
    use crate::api::gateway_stamp::test_support::stamped_device_headers;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "owner-phone", Some("Mozilla/5.0"), None)
        .await
        .unwrap();
    let thread = agent_thread(&bus).await;
    let request_id = prepare(&bus, thread, create("tour-film-a")).await.unwrap();
    let card = ask_prepared(&bus, thread, &request_id).await.tool_use_id;
    let bus_ref = &bus;
    let refusal = |headers: HeaderMap| {
        let (pool, card) = (pool.clone(), card.clone());
        async move {
            answering_actor(&pool, bus_ref, &headers, thread, &card)
                .await
                .unwrap_err()
                .status
        }
    };

    assert_eq!(
        refusal(agent_headers(Some(thread))).await,
        StatusCode::FORBIDDEN
    );
    let mut typed = HeaderMap::new();
    typed.insert(
        crate::core::subprocess_origin::HEADER_DEVICE_ID,
        "owner-phone".parse().unwrap(),
    );
    assert_eq!(
        refusal(typed).await,
        StatusCode::FORBIDDEN,
        "an unstamped id"
    );
    let owner = answering_actor(
        &pool,
        &bus,
        &stamped_device_headers("owner-phone"),
        thread,
        &card,
    )
    .await
    .unwrap();
    assert!(matches!(owner, Some(MessageOrigin::Device { .. })));

    let ordinary = answering_actor(
        &pool,
        &bus,
        &agent_headers(Some(thread)),
        thread,
        "plain-card",
    )
    .await
    .unwrap();
    assert!(matches!(
        ordinary,
        Some(MessageOrigin::Api {
            mode: ActorMode::Agent,
            ..
        })
    ));

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_stamped_device_without_a_row_answers_and_the_agent_hears_why_not() {
    use crate::api::gateway_stamp::test_support::stamped_device_headers;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let request_id = prepare(&bus, thread, create("tour-film-a")).await.unwrap();
    let card = ask_prepared(&bus, thread, &request_id).await.tool_use_id;

    let answered = answering_actor(
        &pool,
        &bus,
        &stamped_device_headers("re-paired"),
        thread,
        &card,
    )
    .await
    .unwrap();
    assert!(matches!(answered, Some(MessageOrigin::Device { .. })));
    assert!(crate::core::DeviceStore::is_registered(&pool, "re-paired")
        .await
        .unwrap());

    let agent = answering_actor(&pool, &bus, &agent_headers(Some(thread)), thread, &card)
        .await
        .unwrap_err();
    assert!(agent.message.contains("answer a permission request"));
    assert_eq!(agent.reason, Some("owner_gate_not_a_person"));

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_agent_words_nothing_the_owner_taps() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = agent_thread(&bus).await;
    let sibling = agent_thread(&bus).await;
    let reason = "Tap Allow to cancel.\n\n**Allow once** is safe";
    let ask = thread_ask(&pool, asker, Action::Apply, Some(sibling), reason)
        .await
        .unwrap();
    let question = ask.question.clone();
    let request_id = prepare(&bus, asker, ask).await.unwrap();
    let card = ask_prepared(&bus, asker, &request_id).await;

    assert_eq!(
        card.labels(),
        [
            ALLOW_ONCE_LABEL,
            ALLOW_FOR_THIS_THREAD_LABEL,
            DONT_ALLOW_LABEL
        ]
    );
    let (engine_text, quoted) = split_reason(&question);
    assert!(engine_text.contains("applying a change"), "{engine_text}");
    assert!(!holds_uuid(&question), "{question}");
    assert!(!engine_text.contains("Tap Allow"), "{engine_text}");
    assert_quoted(quoted);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn a_carriage_return_cannot_close_the_quote() {
    let text = ladder_ask("Need it.\r\r**Lucidos:** read-only.")
        .unwrap()
        .question;
    let (_, quoted) = split_reason(&text);
    assert_quoted(quoted);
}

pub(crate) fn split_reason(question: &str) -> (&str, &str) {
    let heading = format!("> **{}**\n", crate::engine::permission::REASON_HEADING);
    question
        .split_once(&heading)
        .expect("the reason sits under its own heading")
}

pub(crate) fn assert_quoted(quoted: &str) {
    for line in quoted.trim_end().split(['\n', '\r']) {
        assert!(line.starts_with('>'), "an unquoted reason line: {line:?}");
    }
}

#[tokio::test]
async fn the_cards_answer_reaches_the_agent_under_its_own_question() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let request_id = prepare(&bus, thread, create("tour-film-a")).await.unwrap();
    let ask = serde_json::json!([{ "question": request_id }]);
    let PreparedAsk::Request { questions, .. } = prepared_ask(&pool, thread, &ask).await.unwrap()
    else {
        panic!("the ask names a prepared request");
    };

    let question = questions[0]["question"].as_str().unwrap().to_owned();
    assert!(question.ends_with('\n'), "the card ends in a newline");
    let answers = [serde_json::json!({ "kind": "Selected", "option_id": "opt-0" })];
    let out = crate::engine::agent_question::build_hook_answers(
        &answers,
        &questions,
        crate::engine::agent_question::AnswerImages::Unshown,
    );
    assert_eq!(
        out.get(question.as_str()),
        Some(&serde_json::json!(ALLOW_ONCE_LABEL)),
        "{out}"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn declining_a_draft_send_keeps_the_draft() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = agent_thread(&bus).await;
    let sibling = agent_thread(&bus).await;

    let draft = thread_ask(&pool, asker, Action::SubmitDraft, Some(sibling), "Send it.")
        .await
        .unwrap();
    assert!(draft.question.contains("saved draft"), "{}", draft.question);
    assert!(!draft.question.contains("outside its own subtree"));
    assert!(draft.declined.contains("draft stays unsent"));
    let apply = thread_ask(&pool, asker, Action::Apply, Some(sibling), "why")
        .await
        .unwrap();
    assert_eq!(apply.declined, "The thread stays inside its own subtree.");

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn only_a_proposable_verb_at_its_target_with_a_reason_is_asked() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = agent_thread(&bus).await;
    let sibling = agent_thread(&bus).await;
    let db_pool = &pool;
    let status = move |action: Action, target: Option<Uuid>, reason: &'static str| async move {
        thread_ask(db_pool, asker, action, target, reason)
            .await
            .err()
            .map(|e| (e.status, e.message))
    };

    for retired in [Action::CreateTopThread, Action::DetachChild] {
        let (code, message) = status(retired, Some(sibling), "why").await.unwrap();
        assert_eq!(code, StatusCode::CONFLICT, "{retired:?}");
        assert!(message.contains("needs no permission"), "{message}");
    }
    for answer in [Action::AnswerQuestion, Action::ResolvePermission] {
        let (code, _) = status(answer, Some(sibling), "why").await.unwrap();
        assert_eq!(code, StatusCode::BAD_REQUEST, "{answer:?}");
    }
    let (code, _) = status(Action::Cancel, None, "why").await.unwrap();
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, _) = status(Action::Apply, Some(sibling), "  ").await.unwrap();
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(status(Action::Apply, Some(sibling), "why").await.is_none());

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_plan_card_pick_grants_nothing() {
    use crate::api::thread_reach::refuse_without_authority;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let asker = agent_thread(&bus).await;
    let sibling = agent_thread(&bus).await;
    let fresh_session = "Implement in a fresh session: Sonnet 5.5, high (Recommended)";
    let input = serde_json::json!({ "questions": [{
        "question": "Build it?",
        "options": [{ "label": fresh_session }, { "label": "Approve here" }],
    }] });
    let parsed = crate::engine::question_parsing::parse_ask_user_question_inputs(&input).remove(0);
    let tool_use_id = format!("toolu-{}#q0", Uuid::new_v4());
    let meta = |actor| EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        actor,
        ..EventMeta::NONE
    };
    bus.emit(BusEvent::Thread {
        thread_id: asker,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.clone(),
            cc_session_id: String::new(),
            question: parsed.question,
            options: parsed.options.clone(),
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            permission_request: None,
        },
        meta: meta(None),
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: asker,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id,
            quiet: false,
            answer: AnswerKind::Selected {
                option_id: parsed.options[0].id.clone(),
            },
        },
        meta: meta(Some(owner_device())),
    })
    .await
    .unwrap();

    let headers = agent_headers(Some(asker));
    for verb in [
        Action::Apply,
        Action::Archive,
        Action::Cancel,
        Action::Continue,
    ] {
        assert!(
            refuse_without_authority(&bus, &headers, Some(sibling), verb)
                .await
                .is_err(),
            "{verb:?} must not run on a plan card pick"
        );
    }
    refuse_without_authority(&bus, &headers, Some(sibling), Action::SpawnSuccessor)
        .await
        .expect("a retired verb needs no permission");

    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn a_thread_raises_cards_on_itself_only() {
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    let asks = crate::api::internal::asks_for_another_thread;
    assert!(asks(&agent_headers(Some(mine)), theirs));
    assert!(asks(&agent_headers(None), theirs));
    assert!(!asks(&agent_headers(Some(mine)), mine));
    assert!(!asks(&HeaderMap::new(), theirs));
}

pub(crate) async fn ask_batch_on_coding_route(
    engine: crate::engine::types::SharedEngine,
    workspace_path: std::path::PathBuf,
    thread: Uuid,
    tool: &str,
    id: &str,
) -> serde_json::Value {
    use axum::response::IntoResponse;
    let state = permission_test_state(engine, workspace_path).await;
    let response = crate::api::internal::ask_user_question(
        State(state),
        agent_headers(Some(thread)),
        Json(crate::api::internal::AskUserQuestionRequest {
            thread_id: thread.to_string(),
            tool_use_id: tool.into(),
            session_id: "test-session".into(),
            questions: serde_json::json!([{ "question": id }]),
        }),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn an_engine_card_id_is_not_an_unknown_prepared_request() {
    use crate::engine::permission::{raise_request, RequestCard, RequestOption};
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    let id = raise_request(
        &bus,
        thread,
        RequestCard {
            question: "Read the host log?".into(),
            header: "Permission",
            action: Action::CreateWorkspace,
            breadth: None,
            options: vec![RequestOption {
                label: "Don't allow".into(),
                description: String::new(),
                choice: crate::engine::permission::PermissionChoice::Refuse,
            }],
            batch: None,
        },
        EventChannel::Chat,
    )
    .await
    .unwrap();
    let error = prepared_ask(&pool, thread, &serde_json::json!([{"question": id}]))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        crate::engine::permission::prepare::ENGINE_CARD_REFUSAL
    );
    let error = prepared_ask(
        &pool,
        thread,
        &serde_json::json!([{"question": "permission-request:missing"}]),
    )
    .await
    .unwrap_err();
    assert!(error.contains("lucidos ask-permission"), "{error}");
    assert!(error.contains("lucidos permissions batch"), "{error}");
    teardown_test_db(&db).await;
}

pub(crate) async fn permission_test_state(
    engine: crate::engine::types::SharedEngine,
    workspace_path: std::path::PathBuf,
) -> crate::api::AppState {
    let pool = engine.pool().clone();
    let scheduler = crate::scheduler::SchedulerManager::new(engine.clone(), pool.clone())
        .await
        .unwrap();
    crate::api::AppState {
        engine: engine.clone(),
        pool,
        event_store: engine.event_store().clone(),
        embedder: crate::memory::EmbedderSlot::empty(),
        memory_index: None,
        workspace_path,
        app_manager: engine.shared_app_manager(),
        scheduler: std::sync::Arc::new(tokio::sync::Mutex::new(scheduler)),
        started_at: chrono::Utc::now(),
        pending_oauth_flows: Default::default(),
        reveal_tokens: Default::default(),
        socket_tickets: Default::default(),
        event_hooks: std::sync::Arc::new(crate::api::hooks::NoEventHooks),
    }
}

pub(crate) async fn prepare_batch_on_route(
    engine: crate::engine::types::SharedEngine,
    workspace_path: std::path::PathBuf,
    thread: Uuid,
    child: Uuid,
) -> String {
    use tower::ServiceExt;
    let pool = engine.pool().clone();
    let state = permission_test_state(engine, workspace_path).await;
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/permissions/batches")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::json!({
                "items": [{"kind": "card_answer", "thread_id": child, "option": "Go"}]
            })
            .to_string(),
        ))
        .unwrap();
    request.headers_mut().extend(agent_headers(Some(thread)));
    let response = crate::api::permissions::router()
        .with_state(state)
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body.get("tool_use_id").is_none(), "{body}");
    let id = body["request_id"].as_str().unwrap().to_owned();
    let raised: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'UserQuestionAsked'",
    )
    .bind(thread.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(raised, 0, "preparing through HTTP raises no card");
    id
}

#[tokio::test]
async fn durable_device_scope_matches_future_targets_but_no_other_device_or_lifetime() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = agent_thread(&bus).await;
    let target = device_session_target("take-one", "page", 120).unwrap();
    let ask = device_session_ask(&target, "Film a fixture.", "source").unwrap();
    grant(&bus, caller, ask, menu::ALWAYS_ALLOW_LABEL).await;
    let other = agent_thread(&bus).await;
    for (source, device, ttl, allowed) in [
        ("source", "page", 120, true),
        ("foreign", "page", 120, false),
        ("source", "other", 120, false),
        ("source", "page", 119, false),
        ("source", "page", 121, false),
    ] {
        let target = device_session_target("take-two", device, ttl).unwrap();
        let ask = device_session_ask(&target, "Next fixture.", source).unwrap();
        assert_eq!(
            matches!(
                spend_or_prepare(&bus, other, ask).await.unwrap(),
                Spent::DeviceSessionScope
            ),
            allowed
        );
    }
    let target = device_session_target("take-three", "page", 120).unwrap();
    let ask = workspace_ask(Action::GrantDeviceSession, &target, "No provenance.").unwrap();
    assert!(matches!(
        spend_or_prepare(&bus, other, ask).await.unwrap(),
        Spent::Requested(_)
    ));
    let key = crate::engine::permission::GrantBreadth::device_session("source", &target)
        .unwrap()
        .device_scope_key()
        .unwrap();
    let row = live_permission(
        &pool,
        &thread_user(other),
        Action::GrantDeviceSession,
        Some(&key),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        crate::engine::permission::revoke(&bus, row.permission_id, Some(owner_device()))
            .await
            .unwrap(),
        crate::engine::permission::RevokeOutcome::Revoked
    );
    assert!(live_permission(
        &pool,
        &thread_user(other),
        Action::GrantDeviceSession,
        Some(&key)
    )
    .await
    .unwrap()
    .is_none());
    grant(
        &bus,
        caller,
        device_session_ask(&target, "Again.", "source").unwrap(),
        menu::ALWAYS_ALLOW_LABEL,
    )
    .await;
    sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, $2, now(), MAX(sequence) FROM events")
        .bind(Uuid::new_v4()).bind("fixture").execute(&pool).await.unwrap();
    assert!(live_permission(
        &pool,
        &thread_user(other),
        Action::GrantDeviceSession,
        Some(&key)
    )
    .await
    .unwrap()
    .is_none());
    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn device_session_options_explain_future_targets_and_only_offer_stamped_scopes() {
    let target = device_session_target("take-one", "page", 120).unwrap();
    let thread = Uuid::new_v4();
    let once = workspace_ask(Action::GrantDeviceSession, &target, "Fixture.")
        .unwrap()
        .card(thread);
    assert_eq!(once.options.len(), 2);
    let card = device_session_ask(&target, "Fixture.", "source")
        .unwrap()
        .card(thread);
    assert!(!card.question.contains("once?"));
    assert_eq!(card.options.len(), 4);
    for option in &card.options[1..3] {
        for text in [
            "page",
            "120 seconds",
            "current and future",
            "machine-made",
            "source",
        ] {
            assert!(option.description.contains(text), "{}", option.description);
        }
        assert!(matches!(
            &option.choice,
            crate::engine::permission::PermissionChoice::Grant {
                breadth: crate::engine::permission::GrantBreadth::DeviceSession { .. },
                ..
            }
        ));
    }
    assert!(!card.options[2].choice.batch_may_pick());
}

#[tokio::test]
async fn device_session_thread_scope_stays_with_its_holder() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = agent_thread(&bus).await;
    let other = agent_thread(&bus).await;
    let first = device_session_target("take-one", "page", 120).unwrap();
    grant(
        &bus,
        caller,
        device_session_ask(&first, "Fixture.", "source").unwrap(),
        ALLOW_FOR_THIS_THREAD_LABEL,
    )
    .await;
    let next = device_session_target("take-two", "page", 120).unwrap();
    for (thread, allowed) in [(caller, true), (other, false)] {
        let ask = device_session_ask(&next, "Next fixture.", "source").unwrap();
        assert_eq!(
            matches!(
                spend_or_prepare(&bus, thread, ask).await.unwrap(),
                Spent::DeviceSessionScope
            ),
            allowed
        );
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn trigger_grant_requests_refuse_extra_fields() {
    for field in ["definition_digest", "grant_pattern", "folder_metadata"] {
        let mut body = serde_json::json!({
            "action": "grant-trigger-programs",
            "target_trigger_id": "daily",
            "reason": "Review the earlier run.",
        });
        body[field] = serde_json::json!(["Bash(fake:*)"]);
        assert!(serde_json::from_value::<PreparePermissionRequestBody>(body).is_err());
    }
}

#[tokio::test]
async fn trigger_program_grant_uses_only_observed_calls_and_asks_in_the_callers_thread() {
    asked_ahead_program_card(ALWAYS_ALLOW_LABEL, false).await;
}

#[tokio::test]
async fn always_deny_on_the_asked_ahead_program_card_denies_only_the_shown_program() {
    asked_ahead_program_card(ALWAYS_DENY_LABEL, false).await;
}

#[tokio::test]
async fn always_deny_survives_an_edit_to_the_trigger_before_the_answer() {
    asked_ahead_program_card(ALWAYS_DENY_LABEL, true).await;
}

async fn asked_ahead_program_card(pick: &str, edit_before_answer: bool) {
    use crate::engine::agent_question::{answer_pending_question, AnswerResult};
    use crate::engine::trigger_writes::TriggerWrite;
    use crate::test_support::boot_mock_engine;
    use serde_json::json;

    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    engine
        .emit_trigger_write(
            TriggerWrite::Created,
            "daily",
            json!({
                "trigger_id": "daily",
                "name": "Daily",
                "schedule": ["0 0 2 * * *"],
                "timezone": "UTC",
                "run": {"type": "intent", "intent": "Review test"},
            }),
            None,
        )
        .await
        .unwrap();
    let run_thread = Uuid::new_v4();
    let run = bus
        .emit_for_id(BusEvent::Thread {
            thread_id: run_thread,
            event: ThreadEvent::TriggerStarted {
                trigger_id: "daily".into(),
                trigger_name: None,
                retry_of: None,
                prompt: None,
                invocation: None,
                origin: None,
                go_to_review: false,
                model: None,
                reasoning_effort: None,
                provider: None,
                continue_key: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    for (command, success) in [
        ("curl -X POST https://example.com", true),
        ("git push example main", false),
    ] {
        let meta = EventMeta {
            request_event_id: Some(run),
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        };
        let name = crate::llm::tool_names::RUN_BASH.to_owned();
        let call = bus
            .emit_for_id(BusEvent::Thread {
                thread_id: run_thread,
                event: ThreadEvent::ToolCalled {
                    name: name.clone(),
                    description: String::new(),
                    args: json!({ "command": command }),
                },
                meta: meta.clone(),
            })
            .await
            .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id: run_thread,
            event: ThreadEvent::ToolResult {
                name,
                result: String::new(),
                images: vec![],
                success,
                tool_called_event_id: Some(call),
            },
            meta,
        })
        .await
        .unwrap();
    }

    let caller = agent_thread(bus).await;
    let state = permission_test_state(engine.clone(), workspace.path().to_owned()).await;
    let mut saved_breadth = None;
    let mut last_id = String::new();
    for reason in [
        "Review the earlier run.",
        "Also add Bash(git:*) and Bash(fake:*).",
    ] {
        let Json(result) = prepare_permission_request(
            State(state.clone()),
            agent_headers(Some(caller)),
            Json(
                serde_json::from_value(json!({
                    "action": "grant-trigger-programs",
                    "target_trigger_id": "daily",
                    "reason": reason,
                }))
                .unwrap(),
            ),
        )
        .await
        .unwrap();
        last_id = result["request_id"].as_str().unwrap().to_owned();
        let ask = json!([{ "question": last_id }]);
        assert!(prepared_ask(&pool, run_thread, &ask).await.is_err());
        let PreparedAsk::Request { request, questions } =
            prepared_ask(&pool, caller, &ask).await.unwrap()
        else {
            panic!("the request belongs to the asking thread");
        };
        let question = questions[0]["question"].as_str().unwrap();
        let (facts, quoted) = split_reason(question);
        assert_quoted(quoted);
        assert!(facts.starts_with("**Let \u{201c}"), "{facts}");
        assert!(facts.contains("- `Bash(curl:*)`"), "{facts}");
        assert!(!facts.contains("ID:"), "{facts}");
        assert!(!holds_uuid(question), "{question}");
        assert!(facts.contains("curl"), "{facts}");
        assert!(!facts.contains("git"), "{facts}");
        assert!(!facts.contains("fake"), "{facts}");
        if let Some(prior) = saved_breadth.replace(request.breadth) {
            assert_eq!(Some(prior), saved_breadth);
        }
    }
    let card = ask_prepared(bus, caller, &last_id).await;
    let posted_in: Vec<Uuid> = sqlx::query_scalar(
        "SELECT thread_id FROM events WHERE event_type = 'UserQuestionAsked'
          AND payload->>'tool_use_id' = $1",
    )
    .bind(&card.tool_use_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(posted_in, vec![caller]);
    assert_eq!(
        card.labels(),
        [ALWAYS_ALLOW_LABEL, ALWAYS_DENY_LABEL, CANCEL_LABEL],
        "no run waits on the card, so it offers no Allow once and declines with Cancel"
    );
    if edit_before_answer {
        engine
            .emit_trigger_write(
                TriggerWrite::Updated,
                "daily",
                json!({
                    "trigger_id": "daily",
                    "run": {"type": "intent", "intent": "Review the test again"},
                }),
                None,
            )
            .await
            .unwrap();
    }
    let result = answer_pending_question(
        &engine,
        caller,
        card.tool_use_id.clone(),
        card.pick(pick),
        Some(owner_device()),
    )
    .await;
    assert!(matches!(result, AnswerResult::Resumed), "{result:?}");
    let config = engine.trigger_configs.read().unwrap()["daily"].clone();
    let grants: Vec<&str> = config
        .program_grants
        .iter()
        .map(|grant| grant.pattern.as_str())
        .collect();
    let (granted, denied): (&[&str], &[&str]) = if pick == ALWAYS_DENY_LABEL {
        (&[], &["Bash(curl:*)"])
    } else {
        (&["Bash(curl:*)"], &[])
    };
    assert_eq!(grants, granted);
    assert_eq!(config.program_denies, denied);
    assert!(config.folder_denies.is_empty());
    let standing: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'PermissionGranted'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        standing, 0,
        "the answer writes the trigger, not a permission"
    );

    drop(state);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn trigger_folder_grant_uses_a_recorded_refusal_and_saves_only_the_shown_folder() {
    asked_ahead_folder_card(ALWAYS_ALLOW_LABEL).await;
}

#[tokio::test]
async fn always_deny_on_the_asked_ahead_folder_card_denies_only_the_shown_folder() {
    asked_ahead_folder_card(ALWAYS_DENY_LABEL).await;
}

async fn asked_ahead_folder_card(pick: &str) {
    use crate::engine::agent_question::{answer_pending_question, AnswerResult};
    use crate::engine::permission::{
        raise_request, Holder, Lifetime, PermissionChoice, RequestOption,
    };
    use crate::engine::thread_events::permission::batch::{items_digest, Batch, BatchItem};
    use crate::engine::trigger_writes::TriggerWrite;
    use crate::test_support::boot_mock_engine;
    use lucidos_engine::core::trigger_folder_grant::TriggerFolderGrant;
    use serde_json::json;

    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    engine
        .emit_trigger_write(
            TriggerWrite::Created,
            "daily",
            json!({
                "trigger_id": "daily",
                "name": "Daily",
                "schedule": ["0 0 2 * * *"],
                "timezone": "UTC",
                "run": {"type": "intent", "intent": "Review test"},
            }),
            None,
        )
        .await
        .unwrap();
    let refused = tempfile::tempdir().unwrap();
    let grant = TriggerFolderGrant::inspect(refused.path(), workspace.path()).unwrap();
    let run_thread = agent_thread(bus).await;
    let definition_digest = engine.trigger_configs.read().unwrap()["daily"].definition_digest();
    let items = vec![BatchItem::TriggerFolderGrant {
        trigger_id: "daily".into(),
        definition_digest,
        grant: serde_json::to_value(&grant).unwrap(),
        queue_entry_id: Some(Uuid::new_v4()),
        run_thread_id: run_thread,
    }];
    let batch = Batch {
        digest: items_digest(&items).unwrap(),
        items,
    };
    raise_request(
        bus,
        run_thread,
        RequestCard {
            question: "Grant this folder and run again?".into(),
            header: "Folder grant",
            action: Action::RunBatch,
            breadth: None,
            options: vec![RequestOption {
                label: "Grant and run again".into(),
                description: "Grant this folder and run again.".into(),
                choice: PermissionChoice::grant(
                    Lifetime::Once,
                    Holder::Thread {
                        thread_id: run_thread,
                    },
                ),
            }],
            batch: Some(batch),
        },
        EventChannel::Trigger,
    )
    .await
    .unwrap();

    let caller = agent_thread(bus).await;
    let state = permission_test_state(engine.clone(), workspace.path().to_owned()).await;
    let Json(result) = prepare_permission_request(
        State(state),
        agent_headers(Some(caller)),
        Json(
            serde_json::from_value(json!({
                "action": "grant-trigger-folders",
                "target_trigger_id": "daily",
                "reason": "Consider a different folder too.",
            }))
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    let card = ask_prepared(bus, caller, result["request_id"].as_str().unwrap()).await;
    let result = answer_pending_question(
        &engine,
        caller,
        card.tool_use_id.clone(),
        card.pick(pick),
        Some(owner_device()),
    )
    .await;
    assert!(matches!(result, AnswerResult::Resumed), "{result:?}");
    let config = engine.trigger_configs.read().unwrap()["daily"].clone();
    if pick == ALWAYS_DENY_LABEL {
        assert!(config.folder_grants.is_empty(), "a deny grants nothing");
        assert_eq!(config.folder_denies, vec![grant.folder]);
    } else {
        assert_eq!(config.folder_grants, vec![grant]);
        assert!(config.folder_denies.is_empty());
    }
    assert!(config.program_denies.is_empty());
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn trigger_grant_named_folders_reach_the_card_verbatim() {
    use crate::engine::trigger_writes::TriggerWrite;
    use crate::test_support::boot_mock_engine;
    use serde_json::json;

    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    engine
        .emit_trigger_write(
            TriggerWrite::Created,
            "daily",
            json!({
                "trigger_id": "daily", "name": "Daily", "schedule": ["0 0 2 1 1 *"],
                "timezone": "UTC", "run": {"type": "intent", "intent": "Report"},
            }),
            None,
        )
        .await
        .unwrap();
    let caller = agent_thread(bus).await;
    let state = permission_test_state(engine.clone(), workspace.path().to_owned()).await;
    let outside = tempfile::tempdir().unwrap();
    let reports = outside.path().join("Reports `2026`");
    let drafts = outside.path().join("Draft reports");
    std::fs::create_dir(&reports).unwrap();
    std::fs::create_dir(&drafts).unwrap();
    let ask = |folders: serde_json::Value| {
        prepare_permission_request(
            State(state.clone()),
            agent_headers(Some(caller)),
            Json(
                serde_json::from_value(json!({
                    "action": "grant-trigger-folders", "target_trigger_id": "daily",
                    "folders": folders, "reason": "Save the reports.",
                }))
                .unwrap(),
            ),
        )
    };
    for path in [
        "reports".into(),
        format!(
            "{}/../{}",
            outside.path().display(),
            outside.path().file_name().unwrap().to_string_lossy()
        ),
        "/".into(),
        std::env::var("HOME").unwrap(),
    ] {
        let Err(error) = ask(json!([&path])).await else {
            panic!("accepted {path:?}")
        };
        assert_eq!(error.status, StatusCode::BAD_REQUEST, "{path:?}");
    }
    let Json(result) = ask(json!([reports, drafts])).await.unwrap();
    let request_id = result["request_id"].as_str().unwrap();
    let PreparedAsk::Request { questions, .. } =
        prepared_ask(&pool, caller, &json!([{ "question": request_id }]))
            .await
            .unwrap()
    else {
        panic!("expected a prepared grant request")
    };
    let question = questions[0]["question"].as_str().unwrap();
    assert!(question.contains(reports.to_str().unwrap()), "{question}");
    assert!(question.contains(drafts.to_str().unwrap()), "{question}");
    drop(state);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn trigger_grant_requested_at_creation_commits_with_its_answer() {
    use crate::engine::agent_question::{answer_pending_question, AnswerResult};
    use crate::test_support::boot_mock_engine;
    use serde_json::json;

    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let caller = agent_thread(&engine.event_bus).await;
    let state = permission_test_state(engine.clone(), workspace.path().to_owned()).await;
    let outside = tempfile::tempdir().unwrap();
    let folder = outside
        .path()
        .canonicalize()
        .unwrap()
        .join("Reports `2026`");
    std::fs::create_dir(&folder).unwrap();
    let Json(created) = super::super::triggers::create_trigger(
        State(state.clone()),
        agent_headers(Some(caller)),
        Json(
            serde_json::from_value(json!({
                "name": "Daily reports",
                "cron_expressions": ["0 0 2 1 1 *"],
                "run": {"type": "intent", "intent": "Prepare the report"},
                "folder_grants": [&folder],
                "program_grants": ["curl"],
            }))
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert!(created.success, "{:?}", created.error);
    let (queued_in, asked_seq): (Uuid, i64) = sqlx::query_as(
        "SELECT thread_id, sequence FROM events WHERE event_type = 'UserQuestionQueued'",
    )
    .fetch_one(&pool)
    .await
    .expect("the create queued a grant card");
    assert_eq!(queued_in, caller);
    end_chat_turn(&engine.event_bus, caller).await;
    crate::engine::card_queue::release(&engine, caller)
        .await
        .unwrap();
    let (posted_in, payload): (Uuid, serde_json::Value) = sqlx::query_as(
        "SELECT thread_id, payload FROM events WHERE event_type = 'UserQuestionAsked'",
    )
    .fetch_one(&pool)
    .await
    .expect("the turn's end posted the grant card");
    assert_eq!(posted_in, caller);
    let created_seq: i64 = sqlx::query_scalar(
        "SELECT sequence FROM events WHERE event_type = 'TriggerCreated' AND payload->>'name' = 'Daily reports'",
    ).fetch_one(&pool).await.unwrap();
    assert!(
        asked_seq < created_seq,
        "the grant card is recorded before the trigger becomes runnable"
    );
    assert!(payload["question"]
        .as_str()
        .unwrap()
        .contains(folder.to_str().unwrap()));
    assert!(payload["question"].as_str().unwrap().contains("curl"));
    let tool_use_id = payload["tool_use_id"].as_str().unwrap();
    let option_id = payload["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|option| option["label"] == ALWAYS_ALLOW_LABEL)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let allow = || {
        answer_pending_question(
            &engine,
            caller,
            tool_use_id.into(),
            AnswerKind::Selected {
                option_id: option_id.into(),
            },
            Some(owner_device()),
        )
    };
    assert!(engine
        .trigger_configs
        .read()
        .unwrap()
        .values()
        .all(|config| { config.folder_grants.is_empty() && config.program_grants.is_empty() }));

    sqlx::query(
        "ALTER TABLE events ADD CONSTRAINT test_grant_append_fails
                 CHECK (event_type <> 'TriggerUpdated')",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(allow().await, AnswerResult::Conflict(_)));
    let answered: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM events WHERE event_type = 'UserQuestionAnswered'
         AND payload->>'tool_use_id' = $1)",
    )
    .bind(tool_use_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!answered, "a failed grant write must leave the card open");
    sqlx::query("ALTER TABLE events DROP CONSTRAINT test_grant_append_fails")
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(allow().await, AnswerResult::Resumed));

    let config = engine
        .trigger_configs
        .read()
        .unwrap()
        .values()
        .find(|config| config.name == "Daily reports")
        .unwrap()
        .clone();
    assert_eq!(config.folder_grants.len(), 1);
    assert_eq!(config.folder_grants[0].folder, folder);
    assert_eq!(config.program_grants.len(), 1);
    assert_eq!(config.program_grants[0].pattern, "Bash(curl:*)");
    let edit = |folder: &std::path::Path| {
        super::super::triggers::update_trigger(
            State(state.clone()),
            axum::extract::Query(serde_json::from_value(json!({"id": config.id})).unwrap()),
            agent_headers(Some(caller)),
            Json(
                serde_json::from_value(json!({
                    "folder_grants": [folder],
                    "program_grants": ["curl"],
                }))
                .unwrap(),
            ),
        )
    };
    let Json(edited) = edit(&folder).await.unwrap();
    assert!(
        edited.success && edited.warnings.is_none(),
        "{:?}",
        edited.error
    );
    let drafts = outside.path().canonicalize().unwrap().join("Draft reports");
    std::fs::create_dir(&drafts).unwrap();
    let Json(edited) = edit(&drafts).await.unwrap();
    assert!(
        edited.success && edited.warnings.is_some(),
        "{:?}",
        edited.error
    );
    let questions: Vec<String> = sqlx::query_scalar(
        "SELECT payload->>'question' FROM events WHERE event_type = 'UserQuestionAsked'
           AND thread_id = $1 ORDER BY sequence",
    )
    .bind(caller)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(questions.len(), 2);
    assert!(questions[1].contains(drafts.to_str().unwrap()));
    assert!(!questions[1].contains(folder.to_str().unwrap()));
    assert!(!questions[1].contains("curl"));
    drop(state);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn allow_once_on_an_older_trigger_grant_card_saves_nothing() {
    use crate::engine::agent_question::{answer_pending_question, AnswerResult};
    use crate::engine::permission::{raise_request, Action, RequestCard};
    use crate::test_support::boot_mock_engine;
    use serde_json::json;

    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let caller = agent_thread(&engine.event_bus).await;
    end_chat_turn(&engine.event_bus, caller).await;
    let state = permission_test_state(engine.clone(), workspace.path().to_owned()).await;
    let Json(created) = super::super::triggers::create_trigger(
        State(state.clone()),
        agent_headers(Some(caller)),
        Json(
            serde_json::from_value(json!({
                "name": "Daily reports",
                "cron_expressions": ["0 0 2 1 1 *"],
                "run": {"type": "intent", "intent": "Prepare the report"},
                "program_grants": ["curl"],
            }))
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert!(created.success, "{:?}", created.error);
    let card_options = |tool_use_id: String| {
        let pool = pool.clone();
        async move {
            let options: serde_json::Value = sqlx::query_scalar(
                "SELECT payload->'options' FROM events WHERE event_type = 'UserQuestionAsked'
                   AND payload->>'tool_use_id' = $1",
            )
            .bind(tool_use_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            options.as_array().unwrap().clone()
        }
    };
    let (current_id, breadth): (String, String) = sqlx::query_as(
        "SELECT payload->>'tool_use_id', payload->'permission_request'->>'breadth'
           FROM events WHERE event_type = 'UserQuestionAsked'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let pick = |options: &[serde_json::Value], label: &str| AnswerKind::Selected {
        option_id: options
            .iter()
            .find(|option| option["label"] == label)
            .unwrap_or_else(|| panic!("the card offers {label}"))["id"]
            .as_str()
            .unwrap()
            .into(),
    };
    let current = card_options(current_id.clone()).await;
    assert!(matches!(
        answer_pending_question(
            &engine,
            caller,
            current_id,
            pick(&current, CANCEL_LABEL),
            Some(owner_device()),
        )
        .await,
        AnswerResult::Resumed
    ));

    let older_id = raise_request(
        &engine.event_bus,
        caller,
        RequestCard {
            question: "**Grant this access to the trigger?**".into(),
            header: "Permission",
            action: Action::GrantTriggerPrograms,
            breadth: Some(breadth),
            options: vec![crate::engine::permission::RequestOption {
                label: ALLOW_ONCE_LABEL.into(),
                description: "Just this one time.".into(),
                choice: crate::engine::permission::PermissionChoice::grant(
                    crate::engine::permission::Lifetime::Once,
                    crate::engine::permission::Holder::Thread { thread_id: caller },
                ),
            }],
            batch: None,
        },
        EventChannel::Chat,
    )
    .await
    .unwrap();
    let older = card_options(older_id.clone()).await;
    let result = answer_pending_question(
        &engine,
        caller,
        older_id,
        pick(&older, ALLOW_ONCE_LABEL),
        Some(owner_device()),
    )
    .await;
    assert!(matches!(result, AnswerResult::Conflict(_)), "{result:?}");
    assert!(engine
        .trigger_configs
        .read()
        .unwrap()
        .values()
        .all(|config| config.program_grants.is_empty()));

    drop(state);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_card_that_cannot_be_classified_is_refused_not_read_as_ordinary() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = agent_thread(&bus).await;
    pool.close().await;
    let refusal = answering_actor(
        &pool,
        &bus,
        &agent_headers(Some(thread)),
        thread,
        "any-card",
    )
    .await
    .unwrap_err();
    assert_eq!(refusal.status, StatusCode::INTERNAL_SERVER_ERROR);
    teardown_test_db(&db).await;
}

#[test]
fn caller_text_in_a_gateway_request_cannot_close_the_cards_code_fence() {
    let request = r#"{"body":"``` injected"}"#;
    let ask = gateway_control_ask(request, "Turn sharing on.", "why").unwrap();
    assert_eq!(ask.breadth, request);
    assert!(ask.question.contains("\\u0060"), "{}", ask.question);
    assert!(!ask.question.contains("``` injected"), "{}", ask.question);
}

mod guard_tests {
    fn production_files_calling(name: &str) -> Vec<String> {
        let call = format!("{name}(");
        crate::test_support::source_scan::production_sources()
            .into_iter()
            .filter(|(path, body)| path != "api/permission_requests.rs" && body.contains(&call))
            .map(|(path, _)| path)
            .collect()
    }

    #[test]
    fn only_the_gateway_route_builds_a_gateway_control_ask() {
        assert_eq!(
            production_files_calling("gateway_control_ask"),
            ["api/gateway_requests.rs"]
        );
    }

    #[test]
    fn only_the_host_log_broker_builds_a_host_log_ask() {
        assert_eq!(
            production_files_calling("host_log_ask"),
            ["api/host_log_transfer.rs"]
        );
    }
}

#[tokio::test]
async fn thread_archive_honors_the_authority_reported_by_permission_requests() {
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use serde_json::json;
    use tower::ServiceExt;

    let (pool, db) = setup_test_db().await;
    let (engine, workspace) = crate::test_support::boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let caller = agent_thread(bus).await;
    let sibling = agent_thread(bus).await;
    let ancestor = agent_thread(bus).await;
    let granted_target = agent_thread(bus).await;
    let trigger_target = agent_thread(bus).await;
    let child = agent_thread(bus).await;
    let grandchild = agent_thread(bus).await;
    for (id, parent) in [(child, ancestor), (grandchild, child)] {
        sqlx::query("UPDATE thread_summaries SET parent_thread_id = $1 WHERE thread_id = $2")
            .bind(parent)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    for target in [sibling, grandchild, granted_target, trigger_target] {
        end_chat_turn(bus, target).await;
    }

    let state = permission_test_state(engine.clone(), workspace.path().to_owned()).await;
    let app = router()
        .merge(crate::api::threads::router())
        .with_state(state);
    let send = |headers, path: String, body: Body| {
        let mut request = Request::post(path)
            .header("content-type", "application/json")
            .body(body)
            .unwrap();
        request.headers_mut().extend(headers);
        app.clone().oneshot(request)
    };
    let ask = |caller, target| {
        send(
            agent_headers(Some(caller)),
            "/permission-requests".into(),
            Body::from(
                json!({
                    "action": Action::Archive,
                    "target_thread_id": target,
                    "reason": "Archive a settled thread.",
                })
                .to_string(),
            ),
        )
    };
    let archive_with =
        |headers, target| send(headers, format!("/threads/{target}/archive"), Body::empty());
    let archive = |caller, target| archive_with(agent_headers(Some(caller)), target);

    let denied = archive(caller, sibling).await.unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    open_turn(bus, caller, owner_device()).await;
    for (caller, target) in [(caller, sibling), (ancestor, grandchild)] {
        let response = ask(caller, target).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("already hold the authority"));

        let response = archive(caller, target).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let source: String = sqlx::query_scalar(
            "SELECT payload->'actor'->>'source_thread_id'
             FROM events WHERE thread_id = $1 AND event_type = 'ThreadArchived'",
        )
        .bind(target)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(source, caller.to_string());
    }

    let grant_ask = thread_ask(
        &pool,
        ancestor,
        Action::Archive,
        Some(granted_target),
        "Archive it.",
    )
    .await
    .unwrap();
    grant(bus, ancestor, grant_ask, ALLOW_ONCE_LABEL).await;
    assert_eq!(
        archive(ancestor, sibling).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let response = archive(ancestor, granted_target).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(
        archive(ancestor, granted_target).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    let response = archive_with(agent_headers(None), trigger_target)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let trigger_id = "archive-example";
    bus.emit(BusEvent::System(
        crate::engine::event_bus::SystemEvent::TriggerCreated {
            trigger_id: trigger_id.into(),
            payload: json!({ "trigger_id": trigger_id }),
            actor: Some(owner_device()),
        },
    ))
    .await
    .unwrap();
    let token = mint_agent_origin_token(None, 0, Some(trigger_id)).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    let response = archive_with(headers, trigger_target).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let source: String = sqlx::query_scalar(
        "SELECT payload->'actor'->>'emitting_trigger_id'
         FROM events WHERE thread_id = $1 AND event_type = 'ThreadArchived'",
    )
    .bind(trigger_target)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(source, trigger_id);

    drop(app);
    drop(engine);
    teardown_test_db(&db).await;
}
