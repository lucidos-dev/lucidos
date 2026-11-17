//! An unattested caller never passes an owner-only door alone (invariant 3).

use super::*;
use crate::api::actor::{
    init_agent_origin_secret, mint_agent_origin_token, require_attested_owner, user_actor,
    HEADER_AGENT_ORIGIN_TOKEN, HEADER_DEVICE_ID,
};
use crate::api::gateway_stamp::test_support::stamped_device_headers;
use crate::engine::thread_events::ActorMode;

fn typed_device() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_DEVICE_ID, "owner-phone".parse().unwrap());
    headers
}

fn local_token() -> HeaderMap {
    let token = crate::api::local_auth::publish_test_local_token();
    let mut headers = HeaderMap::new();
    headers.insert(
        lucidos_local_token::HEADER_LOCAL_TOKEN,
        token.parse().unwrap(),
    );
    headers
}

fn agent() -> HeaderMap {
    init_agent_origin_secret("owner-door-test-secret".to_string());
    let token = mint_agent_origin_token(Some(Uuid::new_v4()), 0, None).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    headers
}

fn ask_to_delete() -> OnUnattested {
    OnUnattested::Ask(OwnerAct::new("delete a thread"))
}

#[test]
fn attestation_reads_each_credential() {
    assert_eq!(
        owner_attestation(&stamped_device_headers("owner-phone")),
        OwnerAttestation::Owner
    );
    assert_eq!(owner_attestation(&agent()), OwnerAttestation::Agent);
    for unattested in [typed_device(), local_token(), HeaderMap::new()] {
        assert_eq!(
            owner_attestation(&unattested),
            OwnerAttestation::Unattested,
            "{unattested:?}"
        );
    }
}

#[tokio::test]
async fn a_replay_is_the_owner_and_nothing_else_is() {
    assert!(!replaying());
    REPLAYING
        .scope((), async {
            assert!(replaying());
            assert_eq!(owner_attestation(&typed_device()), OwnerAttestation::Owner);
        })
        .await;
    assert!(
        !tokio::spawn(async { replaying() }).await.unwrap(),
        "a spawned task never inherits the replay"
    );
}

#[tokio::test]
async fn an_ask_parks_only_inside_the_slot() {
    let outside = ask(OwnerAct::new("delete a thread"));
    assert_eq!(outside.status, StatusCode::FORBIDDEN);
    assert_eq!(outside.message, NOT_ATTESTED_AS_OWNER);

    let (response, parked) = with_parking_slot(async {
        require_attested_owner(&typed_device(), "agents may not", ask_to_delete())
            .expect_err("an unattested caller never passes")
            .into_response()
    })
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        parked.expect("the door parked its act").act,
        "delete a thread"
    );
}

#[tokio::test]
async fn a_door_refuses_or_passes_without_parking() {
    let (_, parked) = with_parking_slot(async {
        let refused =
            require_attested_owner(&local_token(), "agents may not", OnUnattested::Refuse)
                .expect_err("a refusing door refuses");
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        let agent = require_attested_owner(&agent(), "agents may not", ask_to_delete())
            .expect_err("an agent hears its refusal");
        assert_eq!(agent.message, "agents may not");
        require_attested_owner(
            &stamped_device_headers("owner-phone"),
            "agents may not",
            ask_to_delete(),
        )
        .expect("the stamped device is the owner");
        StatusCode::OK.into_response()
    })
    .await;
    assert!(parked.is_none());
}

#[tokio::test]
async fn reach_verbs_ask_for_changes_and_refuse_card_answers() {
    use crate::api::thread_reach::{refuse_without_authority, ThreadReachError, UnattestedOutcome};
    use crate::engine::permission::Action;
    let bus = crate::test_support::offline_event_bus();
    let target = Uuid::new_v4();

    let (_, parked) = with_parking_slot(async {
        let outcome = refuse_without_authority(&bus, &typed_device(), None, Action::Apply)
            .await
            .expect_err("an unattested Apply waits");
        assert_eq!(
            outcome,
            ThreadReachError::Unattested(UnattestedOutcome::Waiting)
        );
        StatusCode::OK.into_response()
    })
    .await;
    let parked = parked.expect("the Apply parked");
    assert_eq!(
        parked.thread, None,
        "a workspace Apply lands in the home thread"
    );

    for verb in [Action::AnswerQuestion, Action::ResolvePermission] {
        assert_eq!(
            refuse_without_authority(&bus, &local_token(), Some(target), verb).await,
            Err(ThreadReachError::Unattested(UnattestedOutcome::CardAnswer))
        );
        assert!(refuse_without_authority(
            &bus,
            &stamped_device_headers("owner-phone"),
            Some(target),
            verb
        )
        .await
        .is_ok());
    }
    assert_eq!(
        refuse_without_authority(&bus, &typed_device(), None, Action::Discard).await,
        Err(ThreadReachError::Unattested(UnattestedOutcome::Refused)),
        "with no slot open, nothing parks"
    );
    assert!(
        refuse_without_authority(&bus, &typed_device(), Some(target), Action::Archive)
            .await
            .is_ok(),
        "an ordinary verb is the caller's own"
    );
}

#[test]
fn the_replay_presents_the_same_caller() {
    let token = crate::api::local_auth::publish_test_local_token();
    let mut presented = typed_device();
    presented.insert(
        lucidos_local_token::HEADER_LOCAL_TOKEN,
        token.parse().unwrap(),
    );
    presented.insert("user-agent", "curl/8".parse().unwrap());
    let caller = user_actor(&presented, None).unwrap();
    assert!(matches!(
        caller,
        MessageOrigin::UnattestedCaller {
            mode: ActorMode::Human,
            claimed_local_token: true,
            ..
        }
    ));
    let rebuilt = caller_headers(&caller, true);
    assert_eq!(user_actor(&rebuilt, None).as_ref(), Some(&caller));
    assert_eq!(
        rebuilt[axum::http::header::CONTENT_TYPE],
        "application/json"
    );
}

#[tokio::test]
async fn a_replay_runs_the_parked_request_through_the_router() {
    async fn door(headers: HeaderMap, body: String) -> Result<String, ApiError> {
        require_attested_owner(&headers, "agents may not", OnUnattested::Refuse)?;
        Ok(body)
    }
    let router = DoorRouter::default();
    router.set(Router::new().route("/door", axum::routing::post(door)));
    let act = UnattestedAct {
        caller: user_actor(&typed_device(), None).unwrap(),
        request: UnattestedRequest::Http {
            method: "POST".into(),
            path: "/door".into(),
            body: Some("{}".into()),
            restore_generation: None,
        },
    };
    replay(&router, &act)
        .await
        .expect("the replay passes the door");

    let missing = UnattestedAct {
        request: UnattestedRequest::Http {
            method: "POST".into(),
            path: "/elsewhere".into(),
            body: None,
            restore_generation: None,
        },
        ..act
    };
    let error = replay(&router, &missing).await.expect_err("no such route");
    assert!(error.starts_with("404"), "{error}");
}

/// A replay sends the restore generation the caller sent, so the restore
/// fence judges it as it judged the live request.
#[tokio::test]
async fn a_replay_carries_the_restore_generation() {
    async fn door(headers: HeaderMap) -> Result<(), ApiError> {
        let sent = headers.get(super::super::restore_generation::GENERATION);
        if sent.is_some_and(|v| v == "none") {
            Ok(())
        } else {
            Err(ApiError::new(StatusCode::CONFLICT, "no generation"))
        }
    }
    let router = DoorRouter::default();
    router.set(Router::new().route("/door", axum::routing::post(door)));
    let act = UnattestedAct {
        caller: user_actor(&typed_device(), None).unwrap(),
        request: UnattestedRequest::Http {
            method: "POST".into(),
            path: "/door".into(),
            body: None,
            restore_generation: Some("none".into()),
        },
    };
    replay(&router, &act)
        .await
        .expect("the replay sent the generation");
}

#[tokio::test]
async fn an_owner_device_door_asks_about_the_local_token() {
    let pool = crate::test_support::offline_pool();
    let (response, parked) = with_parking_slot(async {
        crate::api::actor::require_owner_device(&local_token(), &pool, ask_to_delete())
            .await
            .expect_err("the local token is not the owner's device")
            .into_response()
    })
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(parked.is_some());
    let refused =
        crate::api::actor::require_owner_device(&local_token(), &pool, OnUnattested::Refuse)
            .await
            .expect_err("a refusing door refuses");
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}
