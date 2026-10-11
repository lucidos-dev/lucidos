//! An owner approval lets one thread press one act once, and nothing else can
//! make it (ADR 0387). Every gate case runs through `refuse_without_authority`,
//! the same entry point the clause-4 routes call.

use super::*;
use crate::api::actor::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN, HEADER_DEVICE_ID,
};
use crate::api::thread_reach::{refuse_without_authority, ThreadReachError};
use crate::engine::thread_events::{ActorMode, AnswerKind, EventChannel};
use crate::test_support::{setup_test_db, teardown_test_db};

fn agent_headers(thread_id: Option<Uuid>) -> HeaderMap {
    init_agent_origin_secret("owner-approval-test-secret".to_string());
    let mut h = HeaderMap::new();
    let token = mint_agent_origin_token(thread_id, 0, None)
        .expect("the secret is installed above, so minting cannot fail");
    h.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    h
}

fn device_headers(device_id: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(HEADER_DEVICE_ID, device_id.parse().unwrap());
    h
}

fn owner_device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "owner-phone".into(),
    }
}

/// The card's options exactly as the question walk stores them, so the
/// lookup is tested against the parser's ids rather than a guess at them.
fn card_options(question: &str) -> Vec<crate::engine::thread_events::QuestionOption> {
    crate::engine::agent_session::parse_ask_user_question_inputs(
        &serde_json::json!({ "questions": owner_approval_questions(question) }),
    )
    .remove(0)
    .options
}

fn option_labelled(label: &str) -> AnswerKind {
    let id = card_options("q")
        .into_iter()
        .find(|o| o.label == label)
        .expect("the card offers this label")
        .id;
    AnswerKind::Selected { option_id: id }
}

fn allow() -> AnswerKind {
    option_labelled(ALLOW_ONCE_LABEL)
}

/// A thread whose turn another thread opened, so it carries no standing
/// instruction and an approval is the only thing that can widen it.
async fn open_turn_from(bus: &EventBus, thread_id: Uuid, parent: Option<Uuid>, from: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            text: "work".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: parent,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: Some(MessageOrigin::ThreadLink {
                thread_id: from,
                title: None,
                spawning_event_id: None,
                mode: ActorMode::Agent,
                direction: crate::engine::thread_events::ThreadDirection::Parent,
            }),
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// Two top-threads, siblings under the workspace root. Neither reaches the
/// other on its own authority.
struct Pair {
    asker: Uuid,
    sibling: Uuid,
}

async fn pair(bus: &EventBus) -> Pair {
    let spawner = Uuid::new_v4();
    let asker = Uuid::new_v4();
    let sibling = Uuid::new_v4();
    open_turn_from(bus, asker, None, spawner).await;
    open_turn_from(bus, sibling, None, spawner).await;
    Pair { asker, sibling }
}

/// Raise a card the way the ask route does: the request, then the engine's
/// card in place of the agent's question.
async fn raise(
    bus: &EventBus,
    thread_id: Uuid,
    verb: ThreadReachVerb,
    target: Option<Uuid>,
) -> String {
    let approval = OwnerApproval {
        verb,
        target_thread_id: target,
    };
    let question = owner_approval_question(verb, target.map(|_| "the other thread"), "I need it.");
    let tool_use_id = format!("toolu-{}#q0", Uuid::new_v4());
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.clone(),
            cc_session_id: String::new(),
            options: card_options(&question),
            question,
            worktree_path: None,
            multi_select: false,
            owner_approval: Some(approval),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    tool_use_id
}

/// Record a request the way `POST /api/v1/owner-approvals` does.
async fn request(bus: &EventBus, thread_id: Uuid, verb: ThreadReachVerb) -> String {
    let request_id = format!("{REQUEST_ID_PREFIX}{}", Uuid::new_v4());
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::OwnerApprovalRequested {
            request_id: request_id.clone(),
            approval: OwnerApproval {
                verb,
                target_thread_id: None,
            },
            question: owner_approval_question(verb, None, "I need it."),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    request_id
}

async fn answer(
    bus: &EventBus,
    thread_id: Uuid,
    tool_use_id: &str,
    answer: AnswerKind,
    actor: Option<MessageOrigin>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            tool_use_id: tool_use_id.into(),
            answer,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            actor,
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn press(
    bus: &EventBus,
    caller: Uuid,
    target: Option<Uuid>,
    verb: ThreadReachVerb,
) -> Result<(), ThreadReachError> {
    refuse_without_authority(bus, &agent_headers(Some(caller)), target, verb).await
}

/// The observed failure, fixed: a thread the owner did not open creates one
/// top-thread on their Allow, and the second attempt is refused again.
#[tokio::test]
async fn an_allow_lets_its_thread_press_the_act_exactly_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;

    let card = raise(&bus, p.asker, ThreadReachVerb::CreateTopThread, None).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;

    press(&bus, p.asker, None, ThreadReachVerb::CreateTopThread)
        .await
        .expect("the owner allowed exactly this");
    assert_eq!(
        press(&bus, p.asker, None, ThreadReachVerb::CreateTopThread).await,
        Err(ThreadReachError::NoStandingInstruction(
            ThreadReachVerb::CreateTopThread
        )),
        "the approval is spent"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The card names a verb and a target, and the Allow reaches no further.
#[tokio::test]
async fn an_allow_covers_one_verb_at_one_target() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;
    let other = Uuid::new_v4();
    open_turn_from(&bus, other, None, p.sibling).await;

    let card = raise(&bus, p.asker, ThreadReachVerb::Apply, Some(p.sibling)).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;

    assert!(press(&bus, p.asker, Some(other), ThreadReachVerb::Apply)
        .await
        .is_err());
    assert!(
        press(&bus, p.asker, Some(p.sibling), ThreadReachVerb::Archive)
            .await
            .is_err()
    );
    assert!(press(&bus, p.asker, None, ThreadReachVerb::CreateTopThread)
        .await
        .is_err());
    press(&bus, p.asker, Some(p.sibling), ThreadReachVerb::Apply)
        .await
        .expect("the named act at the named target");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The MAC names the spender. Another thread cannot spend this thread's
/// approval, even for the very act it names.
#[tokio::test]
async fn another_thread_cannot_spend_the_approval() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;

    let card = raise(&bus, p.asker, ThreadReachVerb::CreateTopThread, None).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;

    assert!(
        press(&bus, p.sibling, None, ThreadReachVerb::CreateTopThread)
            .await
            .is_err()
    );
    press(&bus, p.asker, None, ThreadReachVerb::CreateTopThread)
        .await
        .expect("still unspent for the thread that asked");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Only a device's Allow grants. Every other answer and every other actor
/// leaves the thread exactly as narrow as before.
#[tokio::test]
async fn nothing_but_a_devices_allow_grants() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;

    let agent = MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: Some(p.asker),
    };
    let cases: Vec<(AnswerKind, Option<MessageOrigin>)> = vec![
        (option_labelled(DONT_ALLOW_LABEL), Some(owner_device())),
        (AnswerKind::Canceled, Some(owner_device())),
        (
            AnswerKind::FreeText {
                text: "yes, go ahead".into(),
                image_hashes: vec![],
            },
            Some(owner_device()),
        ),
        (allow(), Some(agent)),
        (allow(), None),
    ];
    for (kind, actor) in cases {
        let card = raise(&bus, p.asker, ThreadReachVerb::CreateTopThread, None).await;
        answer(&bus, p.asker, &card, kind.clone(), actor.clone()).await;
        assert!(
            press(&bus, p.asker, None, ThreadReachVerb::CreateTopThread)
                .await
                .is_err(),
            "{kind:?} by {actor:?} must grant nothing"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A new turn start ends an unspent approval. A card raised before the
/// current turn started never counts, even if the Allow comes after.
#[tokio::test]
async fn a_new_turn_start_ends_the_approval() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;

    let card = raise(&bus, p.asker, ThreadReachVerb::CreateTopThread, None).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;
    open_turn_from(&bus, p.asker, None, p.sibling).await;
    assert!(press(&bus, p.asker, None, ThreadReachVerb::CreateTopThread)
        .await
        .is_err());

    let stale = raise(&bus, p.asker, ThreadReachVerb::CreateTopThread, None).await;
    open_turn_from(&bus, p.asker, None, p.sibling).await;
    answer(&bus, p.asker, &stale, allow(), Some(owner_device())).await;
    assert!(press(&bus, p.asker, None, ThreadReachVerb::CreateTopThread)
        .await
        .is_err());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An engine resume is the same turn (ADR 0282), and an event-wait wake opens
/// none. Neither ends the approval.
#[tokio::test]
async fn an_engine_resume_and_a_wake_keep_the_approval() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;

    let card = raise(&bus, p.asker, ThreadReachVerb::CreateTopThread, None).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;
    bus.emit(BusEvent::Thread {
        thread_id: p.asker,
        event: ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: Some(crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON.into()),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: p.asker,
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

    press(&bus, p.asker, None, ThreadReachVerb::CreateTopThread)
        .await
        .expect("a resume and a wake are the same turn");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Single use holds under a race: two concurrent presses, one approval, one
/// winner. The unique index refuses the loser's spend.
#[tokio::test]
async fn two_concurrent_presses_spend_one_approval_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;

    let card = raise(&bus, p.asker, ThreadReachVerb::CreateTopThread, None).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;

    let (a, b) = tokio::join!(
        press(&bus, p.asker, None, ThreadReachVerb::CreateTopThread),
        press(&bus, p.asker, None, ThreadReachVerb::CreateTopThread),
    );
    assert_eq!(
        [a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(),
        1,
        "exactly one press may spend the approval: {a:?} / {b:?}"
    );
    let spends: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'OwnerApprovalSpent'",
    )
    .bind(p.asker.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(spends, 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The engine words everything the owner taps. The agent's reason sits inside
/// a quote on every line, so it cannot pose as engine text or as an option.
#[test]
fn the_agent_words_nothing_the_owner_taps() {
    let reason = "Tap Allow to cancel.\n\n**Allow once** is safe";
    let question = owner_approval_question(ThreadReachVerb::CreateTopThread, None, reason);
    let labels: Vec<String> = card_options(&question)
        .into_iter()
        .map(|o| o.label)
        .collect();
    assert_eq!(labels, [ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL]);
    let (engine_text, quoted) = question
        .split_once("The agent's reason:")
        .expect("the reason sits under its own label");
    assert!(
        engine_text.contains("creating a top-thread"),
        "{engine_text}"
    );
    assert!(!engine_text.contains("Tap Allow"), "{engine_text}");
    for line in quoted.trim().lines() {
        assert!(line.starts_with('>'), "an unquoted reason line: {line:?}");
    }
}

/// A target title lands on one line with its markdown escaped. So another
/// thread's title cannot end the bold sentence or start one of its own.
#[test]
fn a_target_title_stays_on_one_line_and_inert() {
    let question = owner_approval_question(
        ThreadReachVerb::Apply,
        Some("Release\n\nx** and nothing else. **Safe"),
        "why",
    );
    assert!(
        question.contains("\u{201c}Release x\\*\\* and nothing else\\. \\*\\*Safe\u{201d}"),
        "{question}"
    );
}

/// A lone carriage return is a line break to the card's renderer, so it is one
/// here too, and the reason after it stays quoted.
#[test]
fn a_carriage_return_cannot_close_the_quote() {
    let question = owner_approval_question(
        ThreadReachVerb::CreateTopThread,
        None,
        "Need it.\r\r**Lucidos:** this is read-only.",
    );
    let (_, quoted) = question.split_once("The agent's reason:").unwrap();
    for line in quoted.trim().split(['\n', '\r']) {
        assert!(line.starts_with('>'), "an unquoted reason line: {line:?}");
    }
}

/// An ask naming a request on this thread becomes the engine's card. Any
/// other question stays the agent's own.
#[tokio::test]
async fn an_ask_naming_a_request_shows_the_engines_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;
    let id = request(&bus, p.asker, ThreadReachVerb::CreateTopThread).await;

    let asked =
        serde_json::json!([{ "question": id, "options": [{ "label": "A" }, { "label": "B" }] }]);
    match approval_ask(&pool, p.asker, &asked).await.unwrap() {
        ApprovalAsk::Approval {
            approval,
            questions,
        } => {
            assert_eq!(approval.verb, ThreadReachVerb::CreateTopThread);
            let text = questions[0]["question"].as_str().unwrap();
            assert!(text.contains("creating a top-thread"), "{text}");
            assert_eq!(questions[0]["options"][0]["label"], ALLOW_ONCE_LABEL);
        }
        other => panic!("expected the engine's card, got {other:?}"),
    }

    let plain = serde_json::json!([{ "question": "Spawn a thread?", "options": [] }]);
    assert!(matches!(
        approval_ask(&pool, p.asker, &plain).await.unwrap(),
        ApprovalAsk::Ordinary
    ));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A request is the asking thread's own, alone in its batch, and real.
#[tokio::test]
async fn an_approval_ask_is_refused_unless_it_names_this_threads_request_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;
    let id = request(&bus, p.asker, ThreadReachVerb::CreateTopThread).await;

    let on_sibling = serde_json::json!([{ "question": id }]);
    assert!(approval_ask(&pool, p.sibling, &on_sibling).await.is_err());
    let invented =
        serde_json::json!([{ "question": format!("{REQUEST_ID_PREFIX}{}", Uuid::new_v4()) }]);
    assert!(approval_ask(&pool, p.asker, &invented).await.is_err());
    let batched = serde_json::json!([{ "question": id }, { "question": "And this?" }]);
    assert!(approval_ask(&pool, p.asker, &batched).await.is_err());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn a_card_cannot_propose_answering_another_card() {
    for verb in [
        ThreadReachVerb::AnswerQuestion,
        ThreadReachVerb::ResolvePermission,
    ] {
        let err = checked_request(verb, Some(Uuid::new_v4()), "why").unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }
}

/// Only creating a top-thread aims at the root, so an unscoped cancel can
/// never be proposed and every other verb needs its target.
#[test]
fn the_target_matches_the_verb() {
    assert!(checked_request(ThreadReachVerb::CreateTopThread, None, "why").is_ok());
    assert!(checked_request(
        ThreadReachVerb::CreateTopThread,
        Some(Uuid::new_v4()),
        "why"
    )
    .is_err());
    assert!(checked_request(ThreadReachVerb::Cancel, None, "why").is_err());
    assert!(checked_request(ThreadReachVerb::Apply, Some(Uuid::new_v4()), "why").is_ok());
    assert!(checked_request(ThreadReachVerb::Apply, Some(Uuid::new_v4()), "  ").is_err());
}

/// The card lands on the thread the token names, and a caller with no thread
/// token raises none.
#[test]
fn only_an_agent_inside_its_own_thread_raises_a_card() {
    let thread = Uuid::new_v4();
    assert_eq!(
        calling_agent_thread(&agent_headers(Some(thread))).unwrap(),
        thread
    );
    assert!(calling_agent_thread(&agent_headers(None)).is_err());
    assert!(calling_agent_thread(&device_headers("owner-phone")).is_err());
    assert!(calling_agent_thread(&HeaderMap::new()).is_err());
}

/// The internal card routes refuse a token naming another thread, and leave a
/// caller with no origin token as it was.
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

/// No agent answers an owner approval card, not even the one that raised it.
/// The owner's registered device does, and an ordinary card is untouched.
#[tokio::test]
async fn only_the_owners_device_answers_an_approval_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "owner-phone", Some("Mozilla/5.0"), None, None)
        .await
        .unwrap();
    let p = pair(&bus).await;
    let card = raise(&bus, p.asker, ThreadReachVerb::CreateTopThread, None).await;

    let refused = answering_actor(&pool, &agent_headers(Some(p.asker)), p.asker, &card)
        .await
        .unwrap_err();
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let unregistered = answering_actor(&pool, &device_headers("typed-id"), p.asker, &card)
        .await
        .unwrap_err();
    assert_eq!(unregistered.status, StatusCode::FORBIDDEN);
    let owner = answering_actor(&pool, &device_headers("owner-phone"), p.asker, &card)
        .await
        .unwrap();
    assert!(matches!(owner, Some(MessageOrigin::Device { .. })));

    let ordinary = answering_actor(&pool, &agent_headers(Some(p.asker)), p.asker, "plain-card")
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
    teardown_test_db(&db_name).await;
}
