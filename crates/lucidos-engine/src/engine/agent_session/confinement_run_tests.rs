//! The confinement run's gate: a run starts only under a live permission, a
//! once grant runs once, and an inherited engine runs nothing.

use super::*;
use crate::engine::event_bus::BusEvent;
use crate::engine::permission::menu::DONT_ALLOW_LABEL;
use crate::engine::permission::{
    card_request, resolve_answer, revoke, Holder, Lifetime, PermissionChoice, PermissionRequest,
    PickedBy,
};
use crate::engine::thread_events::{AnswerKind, EventMeta, MessageOrigin, ThreadEvent};
use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};

fn phone() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "phone".into(),
    }
}

async fn gate_for(bus: &EventBus, thread: Uuid, card_id: Option<&str>) -> ConfinementGate {
    let asking = match card_id {
        Some(card_id) => Asking::Waiting { card_id },
        None => Asking::Owner,
    };
    gate(bus, &Machine::Available, thread, asking)
        .await
        .unwrap()
}

/// The card the gate raised, or the test fails.
async fn asked(bus: &EventBus, thread: Uuid) -> String {
    match gate_for(bus, thread, None).await {
        ConfinementGate::Asked { card_id } => card_id,
        other => panic!("expected a card, got {other:?}"),
    }
}

/// The owner taps the option labelled `label`, the way the card's buttons
/// record it: the answer, then what it grants.
async fn tap(bus: &EventBus, thread: Uuid, card_id: &str, label: &str) {
    let (request, answer) = record_answer(bus, thread, card_id, label).await;
    resolve_answer(
        bus,
        thread,
        card_id,
        &request,
        &answer,
        Some(&phone()),
        PickedBy::Tap,
    )
    .await
    .unwrap();
}

/// The first half of a tap: the answer lands, and what it grants has not.
async fn record_answer(
    bus: &EventBus,
    thread: Uuid,
    card_id: &str,
    label: &str,
) -> (PermissionRequest, AnswerKind) {
    let request = card_request(bus.pool(), thread, card_id)
        .await
        .unwrap()
        .expect("the card carries its request");
    let wanted = confinement_card(thread)
        .options
        .into_iter()
        .find(|o| o.label == label)
        .expect("the card offers that label")
        .choice;
    let option_id = request
        .choices
        .iter()
        .find(|(_, choice)| **choice == wanted)
        .map(|(id, _)| id.clone())
        .unwrap();
    let answer = AnswerKind::Selected { option_id };
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: card_id.to_string(),
            answer: answer.clone(),
            quiet: false,
        },
        meta: EventMeta::with_actor(Some(phone())),
    })
    .await
    .unwrap();
    (request, answer)
}

fn permitted(gate: ConfinementGate) -> PermissionRow {
    match gate {
        ConfinementGate::Permitted(permission) => permission,
        other => panic!("expected a permitted run, got {other:?}"),
    }
}

/// No grant: the owner gets one card, and a second call joins it rather than
/// raising another.
#[tokio::test]
async fn no_grant_asks_the_owner_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;

    let card = asked(&bus, thread).await;
    let request = card_request(&pool, thread, &card).await.unwrap().unwrap();
    assert_eq!(request.action, Action::RunConfinementTests);
    assert_eq!(
        asked(&bus, thread).await,
        card,
        "a second call joins the card"
    );
    assert!(matches!(
        gate_for(&bus, thread, Some(&card)).await,
        ConfinementGate::Asked { card_id } if card_id == card
    ));
    teardown_test_db(&db).await;
}

/// A caller asking nobody raises no card, and a grant still runs it.
#[tokio::test]
async fn asking_nobody_raises_no_card() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let quiet = || gate(&bus, &Machine::Available, thread, Asking::Nobody);

    assert!(matches!(
        quiet().await.unwrap(),
        ConfinementGate::Unpermitted
    ));
    assert_eq!(
        tappable_request(&pool, thread, Action::RunConfinementTests)
            .await
            .unwrap(),
        None
    );
    let card = asked(&bus, thread).await;
    tap(&bus, thread, &card, ALWAYS_ALLOW_LABEL).await;
    permitted(quiet().await.unwrap());
    teardown_test_db(&db).await;
}

/// A card a later step overtook has no buttons left, so the next call raises
/// a fresh one rather than joining it.
#[tokio::test]
async fn an_overtaken_card_is_not_joined() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::CodingAgentTextStreamed {
            text: "Polling the suites.".into(),
            coding_agent: crate::runtime::agent_runtime::CodingAgent::ClaudeCode,
            parent_tool_use_id: None,
            progress_note: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert!(matches!(
        gate_for(&bus, thread, Some(&card)).await,
        ConfinementGate::Closed { .. }
    ));
    assert_ne!(asked(&bus, thread).await, card);
    teardown_test_db(&db).await;
}

/// Allow once runs one run. The next call asks again.
#[tokio::test]
async fn a_once_grant_runs_once_then_asks_again() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    tap(&bus, thread, &card, ALLOW_ONCE_LABEL).await;

    let permission = permitted(gate_for(&bus, thread, Some(&card)).await);
    assert_eq!(permission.lifetime, "once");
    let next = asked(&bus, thread).await;
    assert_ne!(next, card, "the spent grant raises a fresh card");
    teardown_test_db(&db).await;
}

/// The answer and its grant are two emits. A poll between them keeps waiting
/// rather than reading the granted card as closed.
#[tokio::test]
async fn a_grant_still_landing_keeps_the_caller_waiting() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    let (request, answer) = record_answer(&bus, thread, &card, ALLOW_ONCE_LABEL).await;

    assert!(matches!(
        gate_for(&bus, thread, Some(&card)).await,
        ConfinementGate::Asked { card_id } if card_id == card
    ));
    resolve_answer(
        &bus,
        thread,
        &card,
        &request,
        &answer,
        Some(&phone()),
        PickedBy::Tap,
    )
    .await
    .unwrap();
    assert_eq!(
        granting_answer(&bus, thread, &card).await.unwrap(),
        Some(GrantState::Landed),
        "a poll that read before the grant re-reads the permission"
    );
    permitted(gate_for(&bus, thread, Some(&card)).await);
    teardown_test_db(&db).await;
}

/// Always allow covers every run, from every thread, until revoked.
#[tokio::test]
async fn a_standing_grant_runs_every_time_until_revoked() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let other = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    tap(&bus, thread, &card, ALWAYS_ALLOW_LABEL).await;

    let permission = permitted(gate_for(&bus, thread, Some(&card)).await);
    permitted(gate_for(&bus, thread, None).await);
    permitted(gate_for(&bus, other, None).await);

    revoke(&pool, &bus, permission.permission_id, Some(phone()))
        .await
        .unwrap();
    asked(&bus, thread).await;
    teardown_test_db(&db).await;
}

/// Allow once belongs to the thread that asked. Another thread gets its own
/// card.
#[tokio::test]
async fn a_once_grant_covers_only_its_own_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let other = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    tap(&bus, thread, &card, ALLOW_ONCE_LABEL).await;

    asked(&bus, other).await;
    permitted(gate_for(&bus, thread, None).await);
    teardown_test_db(&db).await;
}

/// Don't allow closes the card the caller waits on, and runs nothing.
#[tokio::test]
async fn deny_closes_the_card_and_runs_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    tap(&bus, thread, &card, DONT_ALLOW_LABEL).await;
    assert_eq!(granting_answer(&bus, thread, &card).await.unwrap(), None);

    assert!(matches!(
        gate_for(&bus, thread, Some(&card)).await,
        ConfinementGate::Closed { card_id } if card_id == card
    ));
    teardown_test_db(&db).await;
}

/// An agent's answer grants nothing, even when it picks Always allow.
#[tokio::test]
async fn an_agents_answer_grants_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    let request = card_request(&pool, thread, &card).await.unwrap().unwrap();
    for option_id in request.choices.keys() {
        let answer = AnswerKind::Selected {
            option_id: option_id.clone(),
        };
        resolve_answer(&bus, thread, &card, &request, &answer, None, PickedBy::Tap)
            .await
            .unwrap();
    }
    assert!(matches!(
        gate_for(&bus, thread, Some(&card)).await,
        ConfinementGate::Asked { .. }
    ));
    teardown_test_db(&db).await;
}

/// An engine inside an inherited sandbox refuses, and raises no card.
#[tokio::test]
async fn an_inherited_engine_refuses_and_asks_nobody() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;

    let verdict = gate(&bus, &Machine::Inherited, thread, Asking::Owner)
        .await
        .unwrap();
    assert!(matches!(verdict, ConfinementGate::Inherited));
    assert_eq!(
        tappable_request(&pool, thread, Action::RunConfinementTests)
            .await
            .unwrap(),
        None
    );
    teardown_test_db(&db).await;
}

/// The owner asked that the card name the risk, so it never reads like a
/// routine test step.
#[test]
fn the_card_names_the_risk() {
    let card = confinement_card(Uuid::new_v4());
    for words in ["no sandbox", "this branch's code", "Lucidos's secrets"] {
        assert!(card.question.contains(words), "{words}: {}", card.question);
    }
    let always = card
        .options
        .iter()
        .find(|o| o.label == ALWAYS_ALLOW_LABEL)
        .unwrap();
    for words in ["Every future confinement run", "until you revoke it"] {
        assert!(always.description.contains(words), "{words}");
    }
    assert!(matches!(
        always.choice,
        PermissionChoice::Grant {
            lifetime: Lifetime::Standing,
            holder: Holder::Workspace,
            ..
        }
    ));
}
