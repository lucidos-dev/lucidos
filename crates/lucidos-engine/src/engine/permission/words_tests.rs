use std::collections::HashMap;

use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::permission::{
    raise_request, resolve_answer, Holder, Lifetime, RequestCard, RequestOption,
};
use crate::engine::thread_events::EventChannel;
use crate::llm::judgment::{Answer, ChoiceAnswer, Judgment};
use crate::test_support::{message_received, seed_coding_thread, setup_test_db, teardown_test_db};

fn device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "phone".into(),
    }
}

fn typed(text: &str, origin: Option<MessageOrigin>) -> ThreadEvent {
    let mut event = message_received(text);
    if let ThreadEvent::MessageReceived { origin: slot, .. } = &mut event {
        *slot = origin;
    }
    event
}

fn spoken(text: &str) -> ThreadEvent {
    ThreadEvent::SpokenMessageReceived {
        session_id: Uuid::new_v4(),
        text: text.into(),
    }
}

fn words_of(event: &ThreadEvent, meta: &EventMeta) -> Option<OwnerWords> {
    OwnerWords::from_event(Uuid::new_v4(), Uuid::new_v4(), 7, event, meta)
}

/// A person's typed message and the caller's own transcript both count, each
/// from a device.
#[test]
fn typed_and_spoken_words_from_a_device_count() {
    let typed = words_of(&typed("yes, continue", Some(device())), &EventMeta::NONE)
        .expect("a typed message from a device counts");
    assert_eq!(typed.heard, Heard::Typed);
    assert_eq!(typed.device_id, "phone");

    let said = words_of(&spoken("go ahead"), &EventMeta::with_actor(Some(device())))
        .expect("the caller's transcript counts");
    assert_eq!(said.heard, Heard::Spoken);
    assert_eq!(said.text, "go ahead");
}

/// Words no person typed or said from a device count for nothing: an agent's
/// message, a talker's relay of a call, an unattested caller and a blank row.
#[test]
fn words_from_no_person_count_for_nothing() {
    let mut by_agent = typed("yes", Some(device()));
    if let ThreadEvent::MessageReceived { mode, .. } = &mut by_agent {
        *mode = ActorMode::Agent;
    }
    let mut relayed = typed("yes", Some(device()));
    if let ThreadEvent::MessageReceived {
        voice_session_id, ..
    } = &mut relayed
    {
        *voice_session_id = Some(Uuid::new_v4());
    }
    let unattested = MessageOrigin::UnattestedCaller {
        mode: ActorMode::Human,
        user_agent: None,
        claimed_device_id: Some("phone".into()),
        claimed_local_token: false,
        claimed_workspace: None,
    };
    let relay = ThreadEvent::WorkDelegated {
        session_id: Uuid::new_v4(),
        reason: "the caller said yes".into(),
    };

    for (event, meta) in [
        (by_agent, EventMeta::NONE),
        (relayed, EventMeta::NONE),
        (typed("yes", None), EventMeta::with_actor(Some(device()))),
        (typed("yes", Some(unattested.clone())), EventMeta::NONE),
        (typed("   ", Some(device())), EventMeta::NONE),
        (spoken("yes"), EventMeta::NONE),
        (
            spoken("yes"),
            EventMeta::with_actor(Some(unattested.clone())),
        ),
        (relay, EventMeta::with_actor(Some(device()))),
    ] {
        assert_eq!(words_of(&event, &meta), None, "{event:?}");
    }
}

fn option(id: &str, label: &str) -> QuestionOption {
    QuestionOption {
        id: id.into(),
        label: label.into(),
        description: Some(format!("{label}, as the engine words it.")),
        preview: None,
        widget: None,
        quiet: false,
        quiet_reply: None,
    }
}

/// A request offering once, standing and a refusal, as `words` meets it.
fn open_request() -> OpenRequest {
    let thread_id = Uuid::new_v4();
    OpenRequest {
        card: OpenCard::Request {
            tool_use_id: "permission-request:test".into(),
        },
        question: "Continue hardening?".into(),
        options: vec![
            option("opt-0", "Continue hardening"),
            option("opt-1", "Always allow"),
            option("opt-2", "Keep paused"),
            option("opt-9", "Untyped"),
        ],
        request: PermissionRequest {
            action: super::super::Action::Continue,
            breadth: None,
            prepared: None,
            choices: [
                (
                    "opt-0".to_string(),
                    PermissionChoice::grant(Lifetime::Once, Holder::Thread { thread_id }),
                ),
                (
                    "opt-1".to_string(),
                    PermissionChoice::grant(Lifetime::Standing, Holder::Workspace),
                ),
                ("opt-2".to_string(), PermissionChoice::Refuse),
            ]
            .into_iter()
            .collect(),
            batch: None,
            lines: None,
            unattested_act: None,
        },
    }
}

fn answers(choice: &str, probability: f64) -> Answers {
    let probabilities: HashMap<String, f64> = [(choice.to_string(), probability)].into();
    Answers::new(
        [(
            QUESTION_ID.to_string(),
            Answer::Choice(ChoiceAnswer {
                choice: choice.into(),
                probabilities,
                confidence: probability,
            }),
        )]
        .into(),
    )
}

/// The question offers each option the request types, plus one that picks
/// nothing. A lasting option says it needs words asking for that reach.
#[test]
fn the_question_offers_the_typed_options_and_none() {
    let questions = judgment_question(&open_request());
    let Question::Choice { criteria, .. } = &questions[0].1 else {
        panic!("a choice question");
    };
    let ids: Vec<&str> = criteria.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["opt-0", "opt-1", "opt-2", NO_PICK]);
    let rubric = |id: &str| {
        criteria
            .iter()
            .find(|(c, _)| c == id)
            .and_then(|(_, r)| r.clone())
            .unwrap()
    };
    assert!(rubric("opt-1").contains(LASTING_NOTE));
    assert!(!rubric("opt-0").contains(LASTING_NOTE));
}

/// Only a confident pick of a typed option picks it. An unsure pick, the
/// no-pick answer, an untyped option and a missing answer pick nothing.
#[test]
fn only_a_confident_pick_of_a_typed_option_counts() {
    let open = open_request();
    assert_eq!(
        picked_option(&answers("opt-0", 0.97), &open).as_deref(),
        Some("opt-0")
    );
    assert_eq!(
        picked_option(&answers("opt-2", 0.95), &open).as_deref(),
        Some("opt-2")
    );
    assert_eq!(picked_option(&answers("opt-0", 0.6), &open), None);
    assert_eq!(picked_option(&answers(NO_PICK, 0.99), &open), None);
    assert_eq!(picked_option(&answers("opt-9", 0.99), &open), None);
    assert_eq!(picked_option(&Answers::default(), &open), None);
}

/// A judge that answers the same every time, or fails.
struct Stub(Option<Answers>);

#[async_trait::async_trait]
impl JudgmentProvider for Stub {
    async fn ask(
        &self,
        _state: serde_json::Value,
        _questions: Vec<(String, Question)>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<Judgment, BoxError> {
        match &self.0 {
            Some(answers) => Ok(Judgment {
                answers: answers.clone(),
                ..Judgment::default()
            }),
            None => Err("no model is available".into()),
        }
    }
}

/// Raise a request on `thread` offering a once grant and a refusal.
async fn raise(bus: &EventBus, thread: Uuid) -> String {
    let card = RequestCard {
        question: "Continue hardening?".into(),
        header: "Hardening",
        action: super::super::Action::Continue,
        breadth: None,
        options: vec![
            RequestOption {
                label: "Continue hardening".into(),
                description: "Run it once.".into(),
                choice: PermissionChoice::grant(
                    Lifetime::Once,
                    Holder::Thread { thread_id: thread },
                ),
            },
            RequestOption {
                label: "Keep paused".into(),
                description: "Do nothing.".into(),
                choice: PermissionChoice::Refuse,
            },
        ],
        batch: None,
        unattested_act: None,
    };
    raise_request(bus, thread, card, EventChannel::ClaudeCode)
        .await
        .unwrap()
}

async fn emit_words(bus: &EventBus, thread: Uuid, text: &str) -> OwnerWords {
    let event = typed(text, Some(device()));
    let emitted = bus
        .emit(BusEvent::Thread {
            thread_id: thread,
            event: event.clone(),
            meta: EventMeta::NONE,
        })
        .await
        .unwrap()
        .expect("the message persists");
    OwnerWords::from_event(
        thread,
        emitted.event_id,
        emitted.seq,
        &event,
        &EventMeta::NONE,
    )
    .unwrap()
}

/// Words answer only a request asked before them, and only while it is open.
#[tokio::test]
async fn words_answer_only_a_request_asked_before_them() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;

    let early = emit_words(&bus, thread, "yes").await;
    let card = raise(&bus, thread).await;
    let late = emit_words(&bus, thread, "yes").await;

    assert_eq!(
        open_request_before(&pool, thread, early.sequence, early.heard)
            .await
            .unwrap(),
        None,
        "words older than the request never answer it"
    );
    let open = open_request_before(&pool, thread, late.sequence, late.heard)
        .await
        .unwrap()
        .expect("the request is open");
    assert_eq!(
        open.card,
        OpenCard::Request {
            tool_use_id: card.clone()
        }
    );
    assert_eq!(open.options.len(), 2);

    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: card,
            answer: AnswerKind::Selected {
                option_id: "opt-1".into(),
            },
            quiet: true,
        },
        meta: EventMeta::with_actor(Some(device())),
    })
    .await
    .unwrap();
    let after = emit_words(&bus, thread, "yes").await;
    assert_eq!(
        open_request_before(&pool, thread, after.sequence, after.heard)
            .await
            .unwrap(),
        None,
        "an answered request is closed"
    );
    teardown_test_db(&db).await;
}

/// The judgment picks an option only when it is sure. A refusal of the
/// words, an unsure answer and a failed call all pick nothing.
#[tokio::test]
async fn only_a_sure_judgment_picks() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    raise(&bus, thread).await;
    let words = emit_words(&bus, thread, "yes, continue hardening").await;
    let open = open_request_before(&pool, thread, words.sequence, words.heard)
        .await
        .unwrap()
        .unwrap();
    let grant = open
        .request
        .choices
        .iter()
        .find(|(_, c)| matches!(c, PermissionChoice::Grant { .. }))
        .map(|(id, _)| id.clone())
        .unwrap();
    let capture = AuxCapture::new(&bus, thread, ContextPurpose::PermissionAnswer);

    let sure = Stub(Some(answers(&grant, 0.98)));
    let unsure = Stub(Some(answers(&grant, 0.55)));
    let no = Stub(Some(answers(NO_PICK, 0.9)));
    let failing = Stub(None);
    assert_eq!(
        judge_words(&capture, &sure, &open, &words).await,
        Some(grant)
    );
    assert_eq!(judge_words(&capture, &unsure, &open, &words).await, None);
    assert_eq!(judge_words(&capture, &no, &open, &words).await, None);
    assert_eq!(judge_words(&capture, &failing, &open, &words).await, None);
    teardown_test_db(&db).await;
}

/// A grant picked by words records which words picked it, as its receipt.
#[tokio::test]
async fn a_grant_by_words_records_its_receipt() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    raise(&bus, thread).await;
    let words = emit_words(&bus, thread, "go on").await;
    let open = open_request_before(&pool, thread, words.sequence, words.heard)
        .await
        .unwrap()
        .unwrap();
    let grant = open
        .request
        .choices
        .iter()
        .find(|(_, c)| matches!(c, PermissionChoice::Grant { .. }))
        .map(|(id, _)| id.clone())
        .unwrap();

    let OpenCard::Request { tool_use_id } = &open.card else {
        panic!("a permission request's own card");
    };
    resolve_answer(
        &bus,
        std::path::Path::new("/nonexistent/grants"),
        thread,
        tool_use_id,
        &open.request,
        &AnswerKind::Selected { option_id: grant },
        Some(&words.actor()),
        words.picked_by(),
    )
    .await
    .unwrap();

    let receipt: serde_json::Value = sqlx::query_scalar(
        "SELECT payload->'picked_by' FROM events \
         WHERE thread_id = $1 AND event_type = 'PermissionGranted'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        receipt,
        serde_json::json!({ "kind": "typed", "message_event_id": words.event_id })
    );
    teardown_test_db(&db).await;
}

async fn emit_as(bus: &EventBus, thread: Uuid, event: ThreadEvent, meta: EventMeta) -> OwnerWords {
    let emitted = bus
        .emit(BusEvent::Thread {
            thread_id: thread,
            event: event.clone(),
            meta: meta.clone(),
        })
        .await
        .unwrap()
        .expect("the words persist");
    OwnerWords::from_event(thread, emitted.event_id, emitted.seq, &event, &meta).unwrap()
}

/// Only the caller's own transcript reaches a tool-permission card. A typed
/// reply supersedes that card as a denial, so it never answers it. The card
/// reads with every button it draws, an Always allow included.
#[tokio::test]
async fn only_spoken_words_reach_a_tool_permission_card() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    sqlx::query("INSERT INTO thread_summaries (thread_id, source) VALUES ($1, 'chat')")
        .bind(thread)
        .execute(&pool)
        .await
        .unwrap();
    crate::test_support::seed_thread_event(&bus, thread, message_received("push it")).await;
    crate::test_support::seed_thread_event(
        &bus,
        thread,
        ThreadEvent::CommandPermissionRequested {
            request_id: "req-push".into(),
            tool_use_id: "toolu-push".into(),
            tool_name: "run_bash".into(),
            command: "git push origin main".into(),
            summary: "Pushes to a remote.".into(),
            grant_head: None,
            grant_offer: None,
        },
    )
    .await;

    let typed = emit_as(
        &bus,
        thread,
        typed("always allow git", Some(device())),
        EventMeta::NONE,
    )
    .await;
    assert_eq!(
        open_request_before(&pool, thread, typed.sequence, typed.heard)
            .await
            .unwrap(),
        None
    );

    let said = emit_as(
        &bus,
        thread,
        spoken("always allow git"),
        EventMeta::with_actor(Some(device())),
    )
    .await;
    let open = open_request_before(&pool, thread, said.sequence, said.heard)
        .await
        .unwrap()
        .expect("the card is open");
    let OpenCard::Tool(card) = &open.card else {
        panic!("a tool-permission card");
    };
    assert_eq!(card.request_id, "req-push");
    let narrow = open
        .options
        .iter()
        .find(|o| o.label == "Always allow `Bash(git:*)`")
        .expect("the narrow button");
    assert_eq!(
        open.request.choices.get(&narrow.id),
        Some(&PermissionChoice::grant(
            Lifetime::Standing,
            Holder::Workspace
        ))
    );
    let Question::Choice { criteria, .. } = &judgment_question(&open)[0].1 else {
        panic!("one choice question");
    };
    let rubric = |id: &str| {
        criteria
            .iter()
            .find(|(c, _)| c == id)
            .and_then(|(_, r)| r.clone())
            .unwrap()
    };
    assert!(rubric(&narrow.id).contains(LASTING_NOTE));
    assert!(!rubric("legacy-once").contains(LASTING_NOTE));
    teardown_test_db(&db).await;
}
