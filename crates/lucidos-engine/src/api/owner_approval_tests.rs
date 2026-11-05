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

#[test]
fn restore_request_names_exact_workspace_and_snapshot_and_never_own_workspace() {
    let handle = crate::core::snapshot::handle::Handle::generate();
    let body = |workspace: &str, snapshot: &str| RequestOwnerApprovalBody {
        verb: ThreadReachVerb::RestoreSnapshot,
        target_thread_id: None,
        target_workspace_id: Some(workspace.into()),
        snapshot_handle: Some(snapshot.into()),
        reason: "  recover my work\n\n**Allow everything**  ".into(),
    };
    let (approval, question) =
        checked_snapshot_request(&body("target", handle.as_str()), "source").unwrap();
    assert_eq!(approval.target_workspace_id.as_deref(), Some("target"));
    assert_eq!(approval.snapshot_handle.as_deref(), Some(handle.as_str()));
    assert!(approval.target_thread_id.is_none());
    assert!(question.contains(handle.as_str()));
    assert!(question.contains("target"));
    assert!(question.contains("> **Allow everything**"));
    assert!(question.contains("running threads"));
    for workspace in ["source", "", "../target", "Target", "database:target"] {
        assert!(checked_snapshot_request(&body(workspace, handle.as_str()), "source").is_err());
    }
    assert!(checked_snapshot_request(&body("target", "../snapshot"), "source").is_err());
    let mut request = body("target", handle.as_str());
    request.target_thread_id = Some(Uuid::new_v4());
    assert!(checked_snapshot_request(&request, "source").is_err());
    request.target_thread_id = None;
    request.reason = "  ".into();
    assert!(checked_snapshot_request(&request, "source").is_err());
    request.reason = "recover".into();
    request.snapshot_handle = None;
    assert!(checked_snapshot_request(&request, "source").is_err());
}

#[tokio::test]
async fn restore_floor_expires_old_cards_and_requests_even_with_a_later_answer() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;
    let verb = ThreadReachVerb::Apply;
    let old_request = request(&bus, p.asker, verb, p.sibling).await;
    let old_card = raise(&bus, p.asker, verb, p.sibling).await;
    // Test-only observed-boundary fixture. Production writes are journal-bound.
    sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, $2, now(), COALESCE(MAX(sequence),0) FROM events")
        .bind(Uuid::new_v4())
        .bind(crate::core::snapshot::handle::Handle::generate().as_str())
        .execute(&pool).await.unwrap();
    answer(&bus, p.asker, &old_card, allow(), Some(owner_device())).await;
    assert!(!spend_owner_approval(&bus, p.asker, verb, Some(p.sibling)).await);
    let ask = serde_json::json!([{"question": old_request}]);
    assert!(approval_ask(&pool, p.asker, &ask).await.is_err());
    let new_card = raise(&bus, p.asker, verb, p.sibling).await;
    answer(&bus, p.asker, &new_card, allow(), Some(owner_device())).await;
    assert!(spend_owner_approval(&bus, p.asker, verb, Some(p.sibling)).await);
    pool.close().await;
    teardown_test_db(&db).await;
}

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
            wake_mode: None,
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
async fn raise(bus: &EventBus, thread_id: Uuid, verb: ThreadReachVerb, target: Uuid) -> String {
    let approval = OwnerApproval {
        verb,
        target_thread_id: Some(target),
        target_workspace_id: None,
        snapshot_handle: None,
    };
    let question = owner_approval_question(verb, "the other thread", "I need it.");
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
async fn request(bus: &EventBus, thread_id: Uuid, verb: ThreadReachVerb, target: Uuid) -> String {
    let request_id = format!("{REQUEST_ID_PREFIX}{}", Uuid::new_v4());
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::OwnerApprovalRequested {
            request_id: request_id.clone(),
            approval: OwnerApproval {
                verb,
                target_thread_id: Some(target),
                target_workspace_id: None,
                snapshot_handle: None,
            },
            question: owner_approval_question(verb, "the other thread", "I need it."),
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
            quiet: false,
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

/// The act most tests here approve: the asker applying its sibling's change,
/// which no subtree covers.
async fn apply_on_sibling(bus: &EventBus, p: &Pair) -> Result<(), ThreadReachError> {
    press(bus, p.asker, Some(p.sibling), ThreadReachVerb::Apply).await
}

/// A thread the owner did not open applies its sibling's change once on their
/// Allow, and the second attempt is refused again.
#[tokio::test]
async fn an_allow_lets_its_thread_press_the_act_exactly_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;

    let card = raise(&bus, p.asker, ThreadReachVerb::Apply, p.sibling).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;

    apply_on_sibling(&bus, &p)
        .await
        .expect("the owner allowed exactly this");
    assert_eq!(
        apply_on_sibling(&bus, &p).await,
        Err(ThreadReachError::OutOfReach {
            target: p.sibling,
            verb: ThreadReachVerb::Apply
        }),
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

    let card = raise(&bus, p.asker, ThreadReachVerb::Apply, p.sibling).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;

    assert!(press(&bus, p.asker, Some(other), ThreadReachVerb::Apply)
        .await
        .is_err());
    assert!(
        press(&bus, p.asker, Some(p.sibling), ThreadReachVerb::Archive)
            .await
            .is_err()
    );
    press(&bus, p.asker, Some(p.sibling), ThreadReachVerb::Apply)
        .await
        .expect("the named act at the named target");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The retired verbs pass with no card, and no card can be asked for them
/// (ADR 0475). Raising one records nothing.
#[tokio::test]
async fn a_retired_verb_needs_no_approval_and_takes_no_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;
    let child = Uuid::new_v4();
    open_turn_from(&bus, child, Some(p.asker), p.asker).await;

    press(&bus, p.asker, None, ThreadReachVerb::CreateTopThread)
        .await
        .expect("any thread creates a top-thread");
    press(&bus, p.asker, Some(child), ThreadReachVerb::DetachChild)
        .await
        .expect("and moves its own child out");

    for (verb, target) in [
        (ThreadReachVerb::CreateTopThread, None),
        (ThreadReachVerb::DetachChild, Some(child)),
    ] {
        let err = checked_request(verb, target, "why").unwrap_err();
        assert_eq!(err.status, StatusCode::CONFLICT, "{verb:?}");
        assert!(err.message.contains("needs no approval"), "{}", err.message);
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The stored spellings of the retired verbs still read, so an old card's
/// timeline still renders.
#[test]
fn a_retired_verb_still_reads_from_a_stored_card() {
    for (wire, verb) in [
        ("create-top-thread", ThreadReachVerb::CreateTopThread),
        ("detach-child", ThreadReachVerb::DetachChild),
    ] {
        let stored: OwnerApproval =
            serde_json::from_value(serde_json::json!({ "verb": wire })).unwrap();
        assert_eq!(stored.verb, verb);
        assert_eq!(stored.target_thread_id, None);
        assert_eq!(verb_wire(verb), wire);
    }
}

/// The MAC names the spender. Another thread cannot spend this thread's
/// approval, even for the very act it names.
#[tokio::test]
async fn another_thread_cannot_spend_the_approval() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;

    let other = Uuid::new_v4();
    open_turn_from(&bus, other, None, p.sibling).await;
    let card = raise(&bus, p.asker, ThreadReachVerb::Apply, other).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;

    assert!(press(&bus, p.sibling, Some(other), ThreadReachVerb::Apply)
        .await
        .is_err());
    press(&bus, p.asker, Some(other), ThreadReachVerb::Apply)
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
        let card = raise(&bus, p.asker, ThreadReachVerb::Apply, p.sibling).await;
        answer(&bus, p.asker, &card, kind.clone(), actor.clone()).await;
        assert!(
            apply_on_sibling(&bus, &p).await.is_err(),
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

    let card = raise(&bus, p.asker, ThreadReachVerb::Apply, p.sibling).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;
    open_turn_from(&bus, p.asker, None, p.sibling).await;
    assert!(apply_on_sibling(&bus, &p).await.is_err());

    let stale = raise(&bus, p.asker, ThreadReachVerb::Apply, p.sibling).await;
    open_turn_from(&bus, p.asker, None, p.sibling).await;
    answer(&bus, p.asker, &stale, allow(), Some(owner_device())).await;
    assert!(apply_on_sibling(&bus, &p).await.is_err());

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

    let card = raise(&bus, p.asker, ThreadReachVerb::Apply, p.sibling).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;
    bus.emit(BusEvent::Thread {
        thread_id: p.asker,
        event: ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: Some(crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON.into()),
            resumed_abort_id: None,
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

    apply_on_sibling(&bus, &p)
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

    let card = raise(&bus, p.asker, ThreadReachVerb::Apply, p.sibling).await;
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;

    let (a, b) = tokio::join!(apply_on_sibling(&bus, &p), apply_on_sibling(&bus, &p));
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

/// A card as an agent words it. Returns the card's id and its options as the
/// question walk stores them.
async fn raise_card_offering(
    bus: &EventBus,
    thread_id: Uuid,
    labels: &[&str],
) -> (String, Vec<crate::engine::thread_events::QuestionOption>) {
    let options: Vec<_> = labels
        .iter()
        .map(|label| serde_json::json!({ "label": label }))
        .collect();
    let input =
        serde_json::json!({ "questions": [{ "question": "Build it?", "options": options }] });
    let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(&input).remove(0);
    let tool_use_id = format!("toolu-{}#q0", Uuid::new_v4());
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.clone(),
            cc_session_id: String::new(),
            options: parsed.options.clone(),
            question: parsed.question,
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    (tool_use_id, parsed.options)
}

fn pick(options: &[crate::engine::thread_events::QuestionOption], label: &str) -> AnswerKind {
    let option = options
        .iter()
        .find(|o| o.label == label)
        .expect("the card offers this label");
    AnswerKind::Selected {
        option_id: option.id.clone(),
    }
}

/// A plan card's pick grants no authority. A handoff needs none, so its verb
/// is retired, and any other act still needs Allow once on an engine card.
#[tokio::test]
async fn a_plan_card_pick_grants_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;
    let fresh_session = "Implement in a fresh session: Sonnet 5.5, high (Recommended)";
    let (card, options) =
        raise_card_offering(&bus, p.asker, &[fresh_session, "Approve here"]).await;
    answer(
        &bus,
        p.asker,
        &card,
        pick(&options, fresh_session),
        Some(owner_device()),
    )
    .await;

    for verb in [
        ThreadReachVerb::Apply,
        ThreadReachVerb::Archive,
        ThreadReachVerb::Cancel,
        ThreadReachVerb::Continue,
    ] {
        assert!(
            press(&bus, p.asker, Some(p.sibling), verb).await.is_err(),
            "{verb:?} must not spend a plan card pick"
        );
    }
    press(
        &bus,
        p.asker,
        Some(p.sibling),
        ThreadReachVerb::SpawnSuccessor,
    )
    .await
    .expect("a retired verb needs no authority");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The engine words everything the owner taps. The agent's reason sits inside
/// a quote on every line, so it cannot pose as engine text or as an option.
#[test]
fn the_agent_words_nothing_the_owner_taps() {
    let reason = "Tap Allow to cancel.\n\n**Allow once** is safe";
    let question = owner_approval_question(ThreadReachVerb::Apply, "Release", reason);
    let labels: Vec<String> = card_options(&question)
        .into_iter()
        .map(|o| o.label)
        .collect();
    assert_eq!(labels, [ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL]);
    let (engine_text, quoted) = question
        .split_once("The agent's reason:")
        .expect("the reason sits under its own label");
    assert!(engine_text.contains("applying a change"), "{engine_text}");
    assert!(!engine_text.contains("Tap Allow"), "{engine_text}");
    for line in quoted.trim().lines() {
        assert!(line.starts_with('>'), "an unquoted reason line: {line:?}");
    }
}

/// The card's question ends in a newline, because every quoted reason line
/// does. Claude Code echoes the question back verbatim, so the owner's answer
/// is lost unless the hook's answer map carries that exact string as a key.
#[test]
fn the_cards_answer_reaches_the_agent_under_its_own_question() {
    let question = owner_approval_question(ThreadReachVerb::Apply, "Release", "I need it.");
    assert!(question.ends_with('\n'), "the card ends in a newline");
    let questions = owner_approval_questions(&question);
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
}

/// A target title lands on one line with its markdown escaped. So another
/// thread's title cannot end the bold sentence or start one of its own.
#[test]
fn a_target_title_stays_on_one_line_and_inert() {
    let question = owner_approval_question(
        ThreadReachVerb::Apply,
        "Release\n\nx** and nothing else. **Safe",
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
        ThreadReachVerb::Apply,
        "Release",
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
    let id = request(&bus, p.asker, ThreadReachVerb::Apply, p.sibling).await;

    let asked =
        serde_json::json!([{ "question": id, "options": [{ "label": "A" }, { "label": "B" }] }]);
    match approval_ask(&pool, p.asker, &asked).await.unwrap() {
        ApprovalAsk::Approval {
            approval,
            questions,
        } => {
            assert_eq!(approval.verb, ThreadReachVerb::Apply);
            let text = questions[0]["question"].as_str().unwrap();
            assert!(text.contains("applying a change"), "{text}");
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
    let id = request(&bus, p.asker, ThreadReachVerb::Apply, p.sibling).await;

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

/// Every proposable verb names its target, so an unscoped cancel can never be
/// proposed.
#[test]
fn every_proposable_verb_needs_its_target() {
    assert!(checked_request(
        ThreadReachVerb::RestoreSnapshot,
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
    let card = raise(&bus, p.asker, ThreadReachVerb::Apply, p.sibling).await;

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

#[test]
fn legacy_owner_approval_decodes_without_snapshot_authority() {
    let old: OwnerApproval = serde_json::from_value(serde_json::json!({
        "verb": "apply", "target_thread_id": Uuid::new_v4()
    }))
    .unwrap();
    assert!(old.target_workspace_id.is_none());
    assert!(old.snapshot_handle.is_none());
}

#[tokio::test]
async fn snapshot_approval_binds_workspace_handle_caller_and_single_spend() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let p = pair(&bus).await;
    let body = RequestOwnerApprovalBody {
        verb: ThreadReachVerb::RestoreSnapshot,
        target_thread_id: None,
        target_workspace_id: Some("demo".into()),
        snapshot_handle: Some(crate::core::snapshot::handle::Handle::generate().into()),
        reason: "Recover the workspace".into(),
    };
    let (approved, question) = checked_snapshot_request(&body, "source").unwrap();
    let request_id = record_request(&bus, p.asker, approved.clone(), question.clone())
        .await
        .unwrap();
    let ApprovalAsk::Approval {
        approval: raised,
        questions,
    } = approval_ask(
        &pool,
        p.asker,
        &serde_json::json!([{"question": request_id}]),
    )
    .await
    .unwrap()
    else {
        panic!("the stored request must become the engine card")
    };
    assert_eq!(raised.target_workspace_id, approved.target_workspace_id);
    assert_eq!(raised.snapshot_handle, approved.snapshot_handle);
    assert_eq!(questions[0]["question"], question);
    let card = format!("toolu-{}#q0", Uuid::new_v4());
    bus.emit(BusEvent::Thread {
        thread_id: p.asker,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: card.clone(),
            cc_session_id: String::new(),
            options: card_options(&question),
            question,
            worktree_path: None,
            multi_select: false,
            owner_approval: Some(approved.clone()),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert!(!spend_bound_approval(&bus, p.asker, &approved).await);
    answer(&bus, p.asker, &card, allow(), Some(owner_device())).await;
    for wrong in [
        OwnerApproval {
            target_workspace_id: Some("elsewhere".into()),
            ..approved.clone()
        },
        OwnerApproval {
            snapshot_handle: Some(crate::core::snapshot::handle::Handle::generate().into()),
            ..approved.clone()
        },
        OwnerApproval {
            target_thread_id: Some(p.sibling),
            ..approved.clone()
        },
        OwnerApproval {
            target_workspace_id: None,
            ..approved.clone()
        },
        OwnerApproval {
            snapshot_handle: None,
            ..approved.clone()
        },
        OwnerApproval {
            snapshot_handle: Some("../snapshot".into()),
            ..approved.clone()
        },
        OwnerApproval {
            verb: ThreadReachVerb::Apply,
            ..approved.clone()
        },
    ] {
        assert!(!spend_bound_approval(&bus, p.asker, &wrong).await);
    }
    assert!(authority_held(
        &pool,
        Some(p.asker),
        None,
        Some(p.asker),
        ThreadReachVerb::RestoreSnapshot
    )
    .await
    .unwrap()
    .is_none());
    assert!(!spend_bound_approval(&bus, p.sibling, &approved).await);
    assert!(!spend_owner_approval(&bus, p.asker, ThreadReachVerb::RestoreSnapshot, None).await);
    let handle =
        crate::core::snapshot::handle::Handle::try_from(approved.snapshot_handle.clone().unwrap())
            .unwrap();
    assert!(!spend_restore_approval(&bus, p.asker, "demo", "demo", &handle).await);
    assert!(!spend_restore_approval(&bus, p.sibling, "source", "demo", &handle).await);
    let (first, second) = tokio::join!(
        spend_restore_approval(&bus, p.asker, "source", "demo", &handle),
        spend_restore_approval(&bus, p.asker, "source", "demo", &handle),
    );
    assert_ne!(first, second, "exactly one concurrent spend succeeds");
    assert!(!spend_bound_approval(&bus, p.asker, &approved).await);
    let spent: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE aggregate_id = $1 AND event_type = 'OwnerApprovalSpent'",
    )
    .bind(p.asker.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(spent["target_workspace_id"], "demo");
    assert_eq!(spent["snapshot_handle"], approved.snapshot_handle.unwrap());
    assert!(spent.get("target_thread_id").is_none());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Synthetic authority fixture for the combined real-archive restore test.
pub(crate) async fn seed_snapshot_approval(bus: &EventBus) -> (Uuid, Uuid) {
    let p = pair(bus).await;
    let card = raise(bus, p.asker, ThreadReachVerb::Apply, p.sibling).await;
    answer(bus, p.asker, &card, allow(), Some(owner_device())).await;
    (p.asker, p.sibling)
}

pub(crate) async fn assert_snapshot_approval_expired(bus: &EventBus, ids: (Uuid, Uuid)) {
    assert!(!spend_owner_approval(bus, ids.0, ThreadReachVerb::Apply, Some(ids.1)).await);
}
