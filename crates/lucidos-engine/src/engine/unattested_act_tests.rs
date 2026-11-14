//! An unattested caller's card runs its act once, and only a device's Allow
//! once counts (invariant 4).

use super::*;
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

async fn answer(bus: &EventBus, thread_id: Uuid, card: &str, label: &str, actor: MessageOrigin) {
    let option_id = option_id(bus.pool(), thread_id, card, label).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: card.into(),
            answer: AnswerKind::Selected { option_id },
            quiet: true,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            actor: Some(actor),
            ..EventMeta::NONE
        },
    })
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

    let allowed = raise(&bus, thread_id, http_act()).await;
    answer(&bus, thread_id, &allowed, ALLOW_ONCE_LABEL, device()).await;
    let (card, act) = allowed_card(&pool, thread_id, Some(&allowed), false)
        .await
        .unwrap()
        .expect("a device allowed it");
    assert_eq!(card, allowed);
    assert_eq!(act, http_act());
    assert!(spend(&bus, thread_id, &allowed).await);
    assert!(
        !spend(&bus, thread_id, &allowed).await,
        "a card spends once"
    );
    assert!(allowed_card(&pool, thread_id, Some(&allowed), false)
        .await
        .unwrap()
        .is_none());

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
    assert_eq!(card_act(&pool, thread_id, "other").await.unwrap(), None);
    assert!(
        crate::engine::agent_question::is_owner_approval_card(&pool, thread_id, &start)
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
