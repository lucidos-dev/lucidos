//! An unattested caller's card runs its act once, and only a device's Allow
//! once grants it (invariant 1).

use super::*;
use crate::engine::event_bus::BusEvent;
use crate::engine::permission::menu::ALLOW_ONCE_LABEL;
use crate::engine::permission::words::{open_request_before, Heard};
use crate::engine::permission::{GrantBreadth, Holder, Lifetime, PermissionChoice, PickedBy};
use crate::engine::thread_events::{ActorMode, AnswerKind};
use crate::test_support::{setup_test_db, teardown_test_db};

fn caller() -> MessageOrigin {
    MessageOrigin::UnattestedCaller {
        mode: ActorMode::Human,
        user_agent: None,
        claimed_device_id: Some("owner-phone**".into()),
        claimed_local_token: true,
        claimed_workspace: None,
    }
}

fn http_act() -> UnattestedAct {
    UnattestedAct {
        caller: caller(),
        request: UnattestedRequest::Http {
            method: "POST".into(),
            path: "/threads/delete".into(),
            body: Some("{}".into()),
            restore_generation: None,
        },
    }
}

fn start_act() -> UnattestedAct {
    UnattestedAct {
        caller: caller(),
        request: UnattestedRequest::StartCodingAgent,
    }
}

async fn open_thread(bus: &EventBus) -> Uuid {
    let thread_id = Uuid::new_v4();
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
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: Some(caller()),
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    thread_id
}

async fn raise(bus: &EventBus, thread_id: Uuid, act: UnattestedAct) -> String {
    let question = card_question("delete a thread", &act);
    raise_card(bus, thread_id, question, act, EventChannel::Chat)
        .await
        .unwrap()
}

async fn option_id(pool: &sqlx::PgPool, thread_id: Uuid, card: &str, label: &str) -> String {
    sqlx::query_scalar(
        "SELECT o->>'id' FROM events, jsonb_array_elements(payload->'options') o \
         WHERE aggregate_id = $1 AND payload->>'tool_use_id' = $2 AND o->>'label' = $3",
    )
    .bind(thread_id.to_string())
    .bind(card)
    .bind(label)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Pick `label` on `card` as `actor`, and resolve the pick as the answer path
/// does: the answer lands, then the permission request decides what it grants.
pub(crate) async fn answer(
    bus: &EventBus,
    thread_id: Uuid,
    card: &str,
    label: &str,
    actor: MessageOrigin,
) {
    let answer = AnswerKind::Selected {
        option_id: option_id(bus.pool(), thread_id, card, label).await,
    };
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: card.into(),
            answer: answer.clone(),
            quiet: true,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            actor: Some(actor.clone()),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    let request = permission::card_request(bus.pool(), thread_id, card)
        .await
        .unwrap()
        .expect("the card is a permission request");
    permission::resolve_answer(
        bus,
        std::path::Path::new("/nonexistent/grants"),
        thread_id,
        card,
        &request,
        &answer,
        Some(&actor),
        PickedBy::Tap,
    )
    .await
    .unwrap();
}

fn device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "owner-phone".into(),
    }
}

#[tokio::test]
async fn a_device_allow_is_spent_once_and_nothing_else_allows() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = open_thread(&bus).await;

    let refused = raise(&bus, thread_id, http_act()).await;
    answer(&bus, thread_id, &refused, DONT_ALLOW_LABEL, device()).await;
    let forged = raise(&bus, thread_id, http_act()).await;
    answer(&bus, thread_id, &forged, ALLOW_ONCE_LABEL, caller()).await;
    assert!(
        allowed_card(&pool, thread_id, None, false)
            .await
            .unwrap()
            .is_none(),
        "neither Don't allow nor an unattested Allow allows"
    );

    assert_eq!(
        take_allowed_act(&bus, thread_id, &forged).await,
        None,
        "an unattested Allow runs nothing"
    );

    let allowed = raise(&bus, thread_id, http_act()).await;
    answer(&bus, thread_id, &allowed, ALLOW_ONCE_LABEL, device()).await;
    let (permission, act) = allowed_card(&pool, thread_id, Some(&allowed), false)
        .await
        .unwrap()
        .expect("a device allowed it");
    assert_eq!(act, http_act());
    assert_eq!(permission.lifetime, Lifetime::Once.wire());
    assert_eq!(
        take_allowed_act(&bus, thread_id, &allowed).await,
        Some(http_act()),
        "the Allow runs the request"
    );
    assert_eq!(
        take_allowed_act(&bus, thread_id, &allowed).await,
        None,
        "it runs once"
    );
    assert!(
        !permission::spend(&bus, &permission).await,
        "the permission is spent"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The card offers only Allow once and Don't allow. Its grant covers this one
/// act, and the owner's own turns in the thread do not end it.
#[tokio::test]
async fn the_card_grants_one_act_that_outlives_the_owners_turns() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = open_thread(&bus).await;

    let card = raise(&bus, thread_id, http_act()).await;
    let request = permission::card_request(&pool, thread_id, &card)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.action, Action::ActForUnattestedCaller);
    assert_eq!(request.unattested_act, Some(Box::new(http_act())));
    let grants: Vec<_> = request
        .choices
        .values()
        .filter(|c| !matches!(c, PermissionChoice::Refuse))
        .collect();
    assert_eq!(
        grants,
        vec![&PermissionChoice::Grant {
            lifetime: Lifetime::Once,
            holder: Holder::Workspace,
            breadth: GrantBreadth::Asked,
        }]
    );
    let other = raise(&bus, thread_id, http_act()).await;
    let other_request = permission::card_request(&pool, thread_id, &other)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        request.breadth, other_request.breadth,
        "each act is its own"
    );

    crate::api::permission_requests::tests::open_turn(&bus, thread_id, device()).await;
    answer(&bus, thread_id, &card, ALLOW_ONCE_LABEL, device()).await;
    assert!(
        allowed_card(&pool, thread_id, Some(&card), false)
            .await
            .unwrap()
            .is_some(),
        "an Allow after the owner chatted still runs the act"
    );
    assert!(
        allowed_card(&pool, thread_id, Some(&other), false)
            .await
            .unwrap()
            .is_none(),
        "it allows no other card"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Only a tap allows an unattested caller's act: typed or spoken words never
/// answer its card. A card raised before permission requests grants nothing.
#[tokio::test]
async fn only_a_tap_answers_the_card() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = open_thread(&bus).await;

    raise(&bus, thread_id, http_act()).await;
    for heard in [Heard::Typed, Heard::Spoken] {
        assert_eq!(
            open_request_before(&pool, thread_id, i64::MAX, heard)
                .await
                .unwrap(),
            None,
            "{heard:?}"
        );
    }

    let legacy = "unattested-act:legacy";
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: legacy.into(),
            cc_session_id: String::new(),
            question: "Let it?".into(),
            options: crate::engine::agent_session::parse_ask_user_question_inputs(
                &serde_json::json!({ "questions": [{
                    "question": "Let it?",
                    "options": [{ "label": ALLOW_ONCE_LABEL, "description": "" }],
                }] }),
            )
            .remove(0)
            .options,
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            unattested_act: Some(http_act()),
            permission_request: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    let option_id = option_id(&pool, thread_id, legacy, ALLOW_ONCE_LABEL).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: legacy.into(),
            answer: AnswerKind::Selected { option_id },
            quiet: true,
        },
        meta: EventMeta::with_actor(Some(device())),
    })
    .await
    .unwrap();
    assert_eq!(
        card_act(&pool, thread_id, legacy).await.unwrap(),
        Some(http_act()),
        "an old card still reads as the engine's"
    );
    assert_eq!(
        take_allowed_act(&bus, thread_id, legacy).await,
        None,
        "but its Allow grants nothing"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A start card raised before permission requests still parks its start, so
/// no second card is raised. Its Don't allow still closes the parked turn.
#[tokio::test]
async fn an_old_start_card_still_waits_and_its_refusal_closes_the_start() {
    use crate::engine::permission::menu::DONT_ALLOW_LABEL;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = open_thread(&bus).await;
    let legacy = "unattested-act:legacy-start";
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: legacy.into(),
            cc_session_id: String::new(),
            question: "Let it start?".into(),
            options: crate::engine::agent_session::parse_ask_user_question_inputs(
                &serde_json::json!({ "questions": [{
                    "question": "Let it start?",
                    "options": [
                        { "label": ALLOW_ONCE_LABEL, "description": "" },
                        { "label": DONT_ALLOW_LABEL, "description": "" },
                    ],
                }] }),
            )
            .remove(0)
            .options,
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            unattested_act: Some(start_act()),
            permission_request: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert!(start_card_waiting(&pool, thread_id).await.unwrap());

    let dont = option_id(&pool, thread_id, legacy, DONT_ALLOW_LABEL).await;
    close_refused_start(&bus, thread_id, legacy, &dont, Some(device()))
        .await
        .unwrap();
    let refused: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 \
           AND event_type = 'ResponseCanceled'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(refused, 1, "the parked start ends refused");

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_start_spends_only_a_start_card() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = open_thread(&bus).await;

    let request = raise(&bus, thread_id, http_act()).await;
    answer(&bus, thread_id, &request, ALLOW_ONCE_LABEL, device()).await;
    assert!(!start_card_waiting(&pool, thread_id).await.unwrap());
    assert!(!spend_allowed_start(&bus, thread_id).await);
    assert!(
        allowed_card(&pool, thread_id, Some(&request), false)
            .await
            .unwrap()
            .is_some(),
        "a start leaves the request's grant alone"
    );

    let start = raise(&bus, thread_id, start_act()).await;
    assert!(start_card_waiting(&pool, thread_id).await.unwrap());
    answer(&bus, thread_id, &start, ALLOW_ONCE_LABEL, device()).await;
    assert!(!start_card_waiting(&pool, thread_id).await.unwrap());
    assert!(spend_allowed_start(&bus, thread_id).await);
    assert!(
        !spend_allowed_start(&bus, thread_id).await,
        "one Allow, one run"
    );
    assert_eq!(
        card_act(&pool, thread_id, &start).await.unwrap(),
        Some(start_act())
    );
    assert_eq!(
        take_allowed_act(&bus, thread_id, &start).await,
        None,
        "the run spent the start's grant"
    );
    assert_eq!(card_act(&pool, thread_id, "other").await.unwrap(), None);
    assert!(
        crate::engine::agent_question::is_engine_card(&pool, thread_id, &start)
            .await
            .unwrap(),
        "only the owner's device answers it"
    );
    assert_eq!(
        crate::engine::agent_question::lookup_active_question_tool_use_id(&pool, thread_id).await,
        None,
        "a chat message never answers it"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn the_card_escapes_what_the_caller_wrote() {
    let question = card_question("delete a thread", &http_act());
    assert!(question.contains("**delete a thread**"));
    assert!(question.contains("owner\\-phone\\*\\*"));
    assert!(question.contains("presented the machine-local token"));
    assert!(question.contains("POST /api/v1/threads/delete"));
}

/// A start from another workspace names that workspace, escaped, whether the
/// call carried no stamp or a stamped `unattested` verdict.
#[test]
fn the_card_names_the_calling_workspace() {
    let unstamped = MessageOrigin::UnattestedCaller {
        mode: ActorMode::Agent,
        user_agent: None,
        claimed_device_id: None,
        claimed_local_token: false,
        claimed_workspace: Some("my*ws".into()),
    };
    let stamped = MessageOrigin::Workspace {
        workspace: "my*ws".into(),
        thread_id: None,
        event_id: None,
        user_agent: None,
        mode: ActorMode::Agent,
        verdict: Some(crate::engine::thread_events::SpawnVerdict::Unattested),
    };
    for (caller, says) in [
        (unstamped, "named the workspace"),
        (stamped, "came from the workspace"),
    ] {
        let act = UnattestedAct {
            caller,
            request: UnattestedRequest::StartCodingAgent,
        };
        let question = card_question("start a coding agent in this thread", &act);
        assert!(question.contains(says), "{question}");
        assert!(question.contains("my\\*ws"), "{question}");
    }
}
