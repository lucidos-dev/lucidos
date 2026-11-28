use super::*;
use crate::api::gateway_stamp::test_support::{
    insert_stamped_device, stamped_device_headers, stamped_workspace_call_headers,
};
use crate::core::hex;
use crate::core::subprocess_origin::{
    build_watch_pid, host_protection_env_vars, init_agent_origin_secret, mint_agent_origin_token,
    subprocess_origin_env_vars, ENV_AGENT_ORIGIN_TOKEN, ENV_SOURCE_THREAD_ID,
};
use crate::engine::thread_events::SpawnVerdict;

fn headers_with(pairs: &[(&str, &str)]) -> axum::http::HeaderMap {
    let mut h = axum::http::HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            axum::http::HeaderValue::from_str(v).unwrap(),
        );
    }
    h
}

fn caller(
    name: &str,
    thread_id: Option<Uuid>,
    event_id: Option<Uuid>,
    mode: ActorMode,
) -> Option<CallerOrigin> {
    Some(CallerOrigin {
        workspace: name.into(),
        thread_id,
        event_id,
        mode,
    })
}

fn allowed_call(name: &str, thread_id: Option<Uuid>) -> axum::http::HeaderMap {
    stamped_workspace_call_headers(name, thread_id, &SpawnVerdict::Allowed)
}

#[test]
fn build_origin_user_with_caller_yields_workspace() {
    let src_thread = Uuid::new_v4();
    let src_event = Uuid::new_v4();
    let mut h = allowed_call("dev", Some(src_thread));
    h.insert("user-agent", "lucidos-engine/0.1".parse().unwrap());
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("dev-1"),
        None,
        None,
        None,
        caller("dev", Some(src_thread), Some(src_event), ActorMode::Human),
    );
    match origin {
        Some(MessageOrigin::Workspace {
            workspace,
            thread_id,
            event_id,
            user_agent,
            mode: _,
            verdict,
            ..
        }) => {
            assert_eq!(workspace, "dev");
            assert_eq!(thread_id, Some(src_thread));
            assert_eq!(event_id, Some(src_event));
            assert_eq!(user_agent.as_deref(), Some("lucidos-engine/0.1"));
            assert_eq!(verdict, Some(SpawnVerdict::Allowed));
        }
        other => panic!("expected Workspace, got {:?}", other),
    }
}

#[test]
fn build_origin_user_caller_takes_precedence_over_device() {
    let h = allowed_call("myws", None);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("dev-1"),
        None,
        None,
        None,
        caller("myws", None, None, ActorMode::Human),
    );
    assert!(matches!(origin, Some(MessageOrigin::Workspace { .. })));
}

#[test]
fn build_origin_user_with_device_id_yields_the_device_id_only() {
    let h = stamped_device_headers("dev-1");
    let origin = build_message_origin(&h, ActorMode::Human, Some("dev-1"), None, None, None, None);
    match origin {
        Some(origin @ MessageOrigin::Device { .. }) => {
            assert_eq!(
                serde_json::to_value(&origin).unwrap(),
                serde_json::json!({ "kind": "device", "device_id": "dev-1" }),
                "the event stores the id and never the device's name"
            );
        }
        other => panic!("expected Device, got {:?}", other),
    }
}

#[test]
fn a_typed_device_id_with_no_stamp_is_an_unattested_caller() {
    let h = headers_with(&[("user-agent", "curl/8")]);
    let origin = build_message_origin(&h, ActorMode::Human, Some("dev-1"), None, None, None, None);
    assert_eq!(
        serde_json::to_value(&origin).unwrap(),
        serde_json::json!({
            "kind": "unattested_caller",
            "mode": "human",
            "user_agent": "curl/8",
            "claimed_device_id": "dev-1",
        }),
    );
}

#[test]
fn a_stamp_for_another_device_is_an_unattested_caller() {
    let h = stamped_device_headers("dev-1");
    let origin = build_message_origin(&h, ActorMode::Human, Some("dev-2"), None, None, None, None);
    assert!(
        matches!(
            &origin,
            Some(MessageOrigin::UnattestedCaller { claimed_device_id: Some(id), .. }) if id == "dev-2"
        ),
        "{origin:?}"
    );
}

#[test]
fn a_forged_stamp_is_an_unattested_caller() {
    let forged = format!(
        "v1.{}.{}",
        lucidos_gateway_stamp::now_unix(),
        "0".repeat(64)
    );
    let h = headers_with(&[(lucidos_gateway_stamp::HEADER_GATEWAY_STAMP, forged.as_str())]);
    let origin = build_message_origin(&h, ActorMode::Human, Some("dev-1"), None, None, None, None);
    assert!(
        matches!(origin, Some(MessageOrigin::UnattestedCaller { .. })),
        "{origin:?}"
    );
}

#[test]
fn a_caller_with_no_credential_yields_no_actor_at_all() {
    let h = headers_with(&[("user-agent", "curl/8")]);
    let origin = build_message_origin(&h, ActorMode::Human, None, None, None, None, None);
    assert_eq!(
        origin, None,
        "an unidentified caller must not be recorded as the user"
    );
}

#[test]
fn the_machine_local_token_alone_is_an_unattested_caller() {
    let token = crate::api::local_auth::publish_test_local_token();
    let h = headers_with(&[
        ("user-agent", "lucidos-cli"),
        (lucidos_local_token::HEADER_LOCAL_TOKEN, token),
    ]);
    let origin = build_message_origin(&h, ActorMode::Human, None, None, None, None, None);
    assert_eq!(
        origin,
        Some(MessageOrigin::UnattestedCaller {
            mode: ActorMode::Engine,
            user_agent: Some("lucidos-cli".into()),
            claimed_device_id: None,
            claimed_local_token: true,
            claimed_workspace: None,
            claimed_thread_id: None,
        })
    );
}

#[test]
fn a_device_outranks_the_machine_local_token() {
    let token = crate::api::local_auth::publish_test_local_token();
    let mut h = headers_with(&[(lucidos_local_token::HEADER_LOCAL_TOKEN, token)]);
    insert_stamped_device(&mut h, "dev-1");
    let origin = build_message_origin(&h, ActorMode::Human, Some("dev-1"), None, None, None, None);
    match origin {
        Some(MessageOrigin::Device { device_id }) => assert_eq!(device_id, "dev-1"),
        other => panic!("expected Device, got {:?}", other),
    }
}

#[test]
fn the_machine_local_token_does_not_attest_a_typed_device() {
    let token = crate::api::local_auth::publish_test_local_token();
    let h = headers_with(&[(lucidos_local_token::HEADER_LOCAL_TOKEN, token)]);
    let origin = build_message_origin(&h, ActorMode::Human, Some("dev-1"), None, None, None, None);
    assert!(
        matches!(
            &origin,
            Some(MessageOrigin::UnattestedCaller {
                mode: ActorMode::Human,
                claimed_device_id: Some(id),
                claimed_local_token: true,
                claimed_workspace: None,
                ..
            }) if id == "dev-1"
        ),
        "{origin:?}"
    );
}

#[test]
fn a_thread_bound_token_outranks_the_machine_local_token() {
    let thread = Uuid::new_v4();
    init_agent_origin_secret("secret-for-actor-tests".into());
    let Some(agent_token) = mint_agent_origin_token(Some(thread), 0, None) else {
        panic!("the secret was just installed");
    };
    let local = crate::api::local_auth::publish_test_local_token();
    let h = headers_with(&[
        (HEADER_AGENT_ORIGIN_TOKEN, agent_token.as_str()),
        (lucidos_local_token::HEADER_LOCAL_TOKEN, local),
    ]);
    let origin = build_message_origin(&h, ActorMode::Human, None, None, None, None, None);
    match origin {
        Some(MessageOrigin::Api {
            mode,
            source_thread_id,
            ..
        }) => {
            assert_eq!(mode, ActorMode::Agent);
            assert_eq!(source_thread_id, Some(thread));
        }
        other => panic!("expected Api, got {:?}", other),
    }
}

#[test]
fn build_origin_human_mode_with_caller_yields_workspace_human_mode() {
    let h = allowed_call("dev", None);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        None,
        None,
        None,
        None,
        caller("dev", None, None, ActorMode::Human),
    );
    match origin {
        Some(MessageOrigin::Workspace {
            workspace, mode, ..
        }) => {
            assert_eq!(workspace, "dev");
            assert_eq!(mode, ActorMode::Human);
        }
        other => panic!("expected Workspace, got {:?}", other),
    }
}

#[test]
fn build_origin_agent_mode_with_caller_yields_workspace_agent_mode() {
    let h = allowed_call("dev", None);
    let origin = build_message_origin(
        &h,
        ActorMode::Agent,
        None,
        None,
        None,
        None,
        caller("dev", None, None, ActorMode::Agent),
    );
    match origin {
        Some(MessageOrigin::Workspace { mode, .. }) => assert_eq!(mode, ActorMode::Agent),
        other => panic!("expected Workspace agent mode, got {:?}", other),
    }
}

#[test]
fn build_origin_engine_mode_with_caller_yields_workspace_engine_mode() {
    let h = allowed_call("dev", None);
    let origin = build_message_origin(
        &h,
        ActorMode::Engine,
        None,
        None,
        None,
        None,
        caller("dev", None, None, ActorMode::Engine),
    );
    match origin {
        Some(MessageOrigin::Workspace { mode, .. }) => assert_eq!(mode, ActorMode::Engine),
        other => panic!("expected Workspace engine mode, got {:?}", other),
    }
}

#[test]
fn an_unstamped_caller_workspace_is_an_unattested_caller() {
    let thread_id = Uuid::new_v4();
    let origin = build_message_origin(
        &headers_with(&[]),
        ActorMode::Agent,
        None,
        None,
        None,
        None,
        caller("dev", Some(thread_id), None, ActorMode::Agent),
    );
    assert_eq!(
        origin,
        Some(MessageOrigin::UnattestedCaller {
            mode: ActorMode::Agent,
            user_agent: None,
            claimed_device_id: None,
            claimed_local_token: false,
            claimed_workspace: Some("dev".into()),
            claimed_thread_id: Some(thread_id),
        })
    );
}

#[test]
fn stacked_claims_beside_a_caller_workspace_keep_every_hint() {
    let token = crate::api::local_auth::publish_test_local_token();
    let h = headers_with(&[(lucidos_local_token::HEADER_LOCAL_TOKEN, token)]);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("typed-device"),
        None,
        None,
        None,
        caller("dev", None, None, ActorMode::Human),
    );
    assert_eq!(
        origin,
        Some(MessageOrigin::UnattestedCaller {
            mode: ActorMode::Human,
            user_agent: None,
            claimed_device_id: Some("typed-device".into()),
            claimed_local_token: true,
            claimed_workspace: Some("dev".into()),
            claimed_thread_id: None,
        })
    );
}

#[test]
fn a_stamp_for_another_caller_workspace_is_unattested() {
    let thread = Uuid::new_v4();
    for (headers, claimed) in [
        (
            allowed_call("work2", Some(thread)),
            caller("dev", Some(thread), None, ActorMode::Agent),
        ),
        (
            allowed_call("dev", Some(Uuid::new_v4())),
            caller("dev", Some(thread), None, ActorMode::Agent),
        ),
    ] {
        let origin =
            build_message_origin(&headers, ActorMode::Agent, None, None, None, None, claimed);
        assert!(
            matches!(
                origin,
                Some(MessageOrigin::UnattestedCaller {
                    claimed_workspace: Some(ref w),
                    ..
                }) if w == "dev"
            ),
            "{origin:?}"
        );
    }
}

#[test]
fn a_stamped_refusal_rides_the_workspace_origin() {
    let refused = SpawnVerdict::Refused {
        reason: "no grant".into(),
    };
    let origin = build_message_origin(
        &stamped_workspace_call_headers("dev", None, &refused),
        ActorMode::Agent,
        None,
        None,
        None,
        None,
        caller("dev", None, None, ActorMode::Agent),
    );
    assert!(
        matches!(origin, Some(MessageOrigin::Workspace { verdict: Some(ref v), .. }) if *v == refused),
        "{origin:?}"
    );
}

#[test]
fn build_origin_agent_mode_with_parent_thread_yields_thread_link_parent_agent_mode() {
    let parent_id = Uuid::new_v4();
    let origin = build_message_origin(
        &headers_with(&[]),
        ActorMode::Agent,
        None,
        Some(parent_id),
        Some("parent".into()),
        None,
        None,
    );
    match origin {
        Some(MessageOrigin::ThreadLink {
            thread_id,
            mode,
            direction,
            ..
        }) => {
            assert_eq!(thread_id, parent_id);
            assert_eq!(mode, ActorMode::Agent);
            assert_eq!(direction, ThreadDirection::Parent);
        }
        other => panic!("expected ThreadLink, got {:?}", other),
    }
}

#[test]
fn build_origin_engine_mode_with_no_parent_yields_none() {
    let origin = build_message_origin(
        &headers_with(&[]),
        ActorMode::Engine,
        None,
        None,
        None,
        None,
        None,
    );
    assert!(origin.is_none());
}

#[test]
fn build_origin_engine_mode_with_parent_thread_yields_thread_link_engine_mode() {
    let parent_id = Uuid::new_v4();
    let origin = build_message_origin(
        &headers_with(&[]),
        ActorMode::Engine,
        None,
        Some(parent_id),
        None,
        None,
        None,
    );
    match origin {
        Some(MessageOrigin::ThreadLink {
            mode, direction, ..
        }) => {
            assert_eq!(mode, ActorMode::Engine);
            assert_eq!(direction, ThreadDirection::Parent);
        }
        other => panic!("expected ThreadLink, got {:?}", other),
    }
}

#[test]
fn user_actor_convenience_passes_through_to_build_message_origin() {
    let h = stamped_device_headers("dev-1");
    let actor = user_actor(&h, Some("dev-1"));
    match actor {
        Some(MessageOrigin::Device { device_id }) => {
            assert_eq!(device_id, "dev-1");
        }
        other => panic!("expected Device, got {:?}", other),
    }
}

#[test]
fn user_actor_falls_back_to_header_device_id_when_no_explicit_id() {
    let h = stamped_device_headers("abcdef0123456789");
    let actor = user_actor(&h, None);
    match actor {
        Some(MessageOrigin::Device { device_id }) => {
            assert_eq!(device_id, "abcdef0123456789");
        }
        other => panic!("expected Device from header, got {:?}", other),
    }
}

#[test]
fn user_actor_explicit_id_takes_precedence_over_header() {
    let h = stamped_device_headers("dev-from-header");
    let actor = user_actor(&h, Some("dev-from-arg"));
    match actor {
        Some(MessageOrigin::UnattestedCaller {
            claimed_device_id, ..
        }) => {
            assert_eq!(
                claimed_device_id.as_deref(),
                Some("dev-from-arg"),
                "the explicit id is the claim, and the stamp does not name it"
            );
        }
        other => panic!("expected an unattested explicit id, got {:?}", other),
    }
}

#[tokio::test]
async fn an_unidentified_caller_resolves_to_nobody_and_is_refused() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let h = headers_with(&[("user-agent", "curl/8")]);

    assert_eq!(user_actor(&h, None), None);

    let refusal = require_user_actor(&h, &pool, None)
        .await
        .expect_err("a bare curl must be refused");
    assert_eq!(refusal.status, axum::http::StatusCode::UNAUTHORIZED);
    assert_eq!(refusal.message, UNIDENTIFIED_CALLER);
    assert_eq!(refusal.reason, None);
    for credential in [
        "thread-bound origin token",
        "machine-local token",
        "x-lucidos-device-id",
        "caller_workspace",
    ] {
        assert!(
            refusal.message.contains(credential),
            "the refusal must name {credential}: {}",
            refusal.message
        );
    }

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn unknown_and_previous_boot_origin_tokens_get_the_restart_diagnostic() {
    use axum::response::IntoResponse;
    use hmac::Mac;

    let (pool, db_name) = crate::test_support::setup_test_db().await;
    install_test_secret();
    let prefix = format!("{}@0@-@-", Uuid::new_v4());
    let mut old_mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(b"previous-boot-secret").unwrap();
    old_mac.update(prefix.as_bytes());
    let previous_boot = format!(
        "{prefix}.{}",
        hex::hex_lower(&old_mac.finalize().into_bytes())
    );
    let unknown = format!("{prefix}.{}", "00".repeat(32));
    for token in [
        previous_boot.as_str(),
        unknown.as_str(),
        "unrecognised-token",
    ] {
        let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, token)]);
        assert_eq!(subprocess_origin(&h), SubprocessOrigin::NotSubprocess);
        let response = require_user_actor(&h, &pool, None)
            .await
            .expect_err("an unverified token grants no identity")
            .into_response();
        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({
                "reason": "origin_token_stale",
                "error": ORIGIN_TOKEN_STALE
            })
        );
    }
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_current_origin_token_still_passes_the_actor_gate() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let thread = Uuid::new_v4();
    let token = token_for(Some(thread));
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
    assert!(matches!(require_user_actor(&h, &pool, None).await.unwrap(),
        MessageOrigin::Api { mode: ActorMode::Agent, source_thread_id: Some(id), .. } if id == thread));

    let mut h = stamped_device_headers("paired-device");
    h.insert(HEADER_AGENT_ORIGIN_TOKEN, "unknown-token".parse().unwrap());
    assert!(matches!(
        require_user_actor(&h, &pool, None).await.unwrap(),
        MessageOrigin::Device { .. }
    ));
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_registered_device_passes_the_same_gate() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "gate-device", Some("Mozilla/5.0"), None)
        .await
        .unwrap();

    let h = stamped_device_headers("gate-device");
    let actor = require_user_actor(&h, &pool, None)
        .await
        .expect("a registered device identifies itself");
    assert!(matches!(actor, MessageOrigin::Device { .. }));

    let typed = headers_with(&[(HEADER_DEVICE_ID, "gate-device")]);
    let actor = require_user_actor(&typed, &pool, None)
        .await
        .expect("a registered id still identifies the caller");
    assert!(
        matches!(actor, MessageOrigin::UnattestedCaller { .. }),
        "{actor:?}"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_stamped_device_needs_no_registration() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let h = stamped_device_headers("fresh-device");
    let actor = require_user_actor(&h, &pool, None)
        .await
        .expect("a stamped device identifies itself");
    assert!(matches!(actor, MessageOrigin::Device { .. }), "{actor:?}");
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_unregistered_device_id_is_not_evidence() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let h = headers_with(&[(HEADER_DEVICE_ID, "never-registered")]);

    assert!(matches!(
        user_actor(&h, None),
        Some(MessageOrigin::UnattestedCaller { .. })
    ));

    let refusal = require_user_actor(&h, &pool, None)
        .await
        .expect_err("an id naming no device is a header, not identity");
    assert_eq!(refusal.status, axum::http::StatusCode::UNAUTHORIZED);

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_database_error_on_the_device_probe_still_identifies_a_typed_id() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let h = headers_with(&[(HEADER_DEVICE_ID, "typed-during-blip")]);

    pool.close().await;
    let actor = require_user_actor(&h, &pool, None)
        .await
        .expect("a database error must not refuse the caller");
    assert!(
        matches!(actor, MessageOrigin::UnattestedCaller { .. }),
        "{actor:?}"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_unregistered_device_id_falls_through_to_the_machine_token() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let token = crate::api::local_auth::publish_test_local_token();
    let h = headers_with(&[
        (HEADER_DEVICE_ID, "never-registered"),
        (lucidos_local_token::HEADER_LOCAL_TOKEN, token),
    ]);

    let actor = require_user_actor(&h, &pool, None)
        .await
        .expect("the machine token still identifies the caller");
    assert_eq!(
        actor,
        crate::api::local_auth::local_process_origin(None),
        "{actor:?}"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_blank_override_does_not_swallow_a_real_device_header() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "blank-test-device", Some("Mozilla/5.0"), None)
        .await
        .unwrap();

    let h = stamped_device_headers("blank-test-device");
    let actor = require_user_actor(&h, &pool, Some("   "))
        .await
        .expect("the header is the surviving source");
    match actor {
        MessageOrigin::Device { device_id, .. } => assert_eq!(device_id, "blank-test-device"),
        other => panic!("expected Device, got {:?}", other),
    }

    crate::test_support::teardown_test_db(&db_name).await;
}

fn install_test_secret() {
    init_agent_origin_secret("subprocess-test-secret".to_string());
}

fn token_for(thread_id: Option<Uuid>) -> String {
    token_for_fire(thread_id, None)
}

fn token_for_fire(thread_id: Option<Uuid>, emitting_trigger_id: Option<&str>) -> String {
    install_test_secret();
    mint_agent_origin_token(thread_id, 0, emitting_trigger_id).expect("secret installed")
}

#[test]
fn subprocess_origin_not_subprocess_with_no_token_header() {
    install_test_secret();
    let h = headers_with(&[("user-agent", "curl/8.7.1")]);
    assert_eq!(subprocess_origin(&h), SubprocessOrigin::NotSubprocess);
}

#[test]
fn subprocess_origin_not_subprocess_with_wrong_token() {
    install_test_secret();
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, "not-the-real-token")]);
    assert_eq!(subprocess_origin(&h), SubprocessOrigin::NotSubprocess);
}

#[test]
fn subprocess_origin_subprocess_with_matching_token_no_thread() {
    let token = token_for(None);
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess {
            source_thread_id: None,
            chain_depth: 0,
            emitting_trigger_id: None,
            event_hook: None,
        }
    );
}

#[test]
fn subprocess_origin_subprocess_with_matching_token_and_thread_id() {
    let source = Uuid::new_v4();
    let token = token_for(Some(source));
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess {
            source_thread_id: Some(source),
            chain_depth: 0,
            emitting_trigger_id: None,
            event_hook: None,
        },
        "the token's own prefix is the source thread; no other input names it"
    );
}

#[test]
fn a_tampered_thread_prefix_is_not_a_subprocess() {
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    let token = token_for(Some(mine));
    let mac = token.rsplit_once('.').expect("minted token has a mac").1;
    let forged = format!("{theirs}@0@-@-.{mac}");

    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &forged)]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::NotSubprocess,
        "re-pointing a valid token at another thread must not authenticate"
    );
}

#[test]
fn a_token_minted_for_one_thread_does_not_authenticate_another() {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let token_a = token_for(Some(a));
    let token_b = token_for(Some(b));
    assert_ne!(
        token_a, token_b,
        "tokens must be per-thread, not per-engine"
    );

    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token_a)]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess {
            source_thread_id: Some(a),
            chain_depth: 0,
            emitting_trigger_id: None,
            event_hook: None,
        },
        "a token authenticates as exactly the thread it was minted for"
    );
}

#[test]
fn a_thread_bound_token_cannot_be_downgraded_to_the_sentinel() {
    let token = token_for(Some(Uuid::new_v4()));
    let mac = token.rsplit_once('.').expect("minted token has a mac").1;
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &format!("-@0@-@-.{mac}"))]);
    assert_eq!(subprocess_origin(&h), SubprocessOrigin::NotSubprocess);
}

#[test]
fn a_malformed_token_is_not_a_subprocess() {
    let source = Uuid::new_v4();
    let token = token_for(Some(source));
    let (prefix, mac) = token.rsplit_once('.').expect("minted token has a mac");

    for (label, candidate) in [
        ("no separator", prefix.to_string()),
        ("empty mac", format!("{prefix}.")),
        ("non-hex mac", format!("{prefix}.{}", "z".repeat(mac.len()))),
        (
            "truncated mac",
            format!("{prefix}.{}", &mac[..mac.len() - 2]),
        ),
        ("empty prefix", format!(".{mac}")),
        ("mac only", mac.to_string()),
    ] {
        let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &candidate)]);
        assert_eq!(
            subprocess_origin(&h),
            SubprocessOrigin::NotSubprocess,
            "{label} must not authenticate"
        );
    }
}

#[test]
fn subprocess_origin_env_vars_binds_the_token_to_the_thread() {
    install_test_secret();
    let tid = Uuid::new_v4();
    let vars = subprocess_origin_env_vars(Some(tid), 0, None);
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();

    let token = map
        .get(ENV_AGENT_ORIGIN_TOKEN)
        .expect("spawn env carries a token");
    assert_eq!(
        token,
        &mint_agent_origin_token(Some(tid), 0, None).expect("secret installed"),
        "the spawn env's token must be the one minted for this thread"
    );
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, token)]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess {
            source_thread_id: Some(tid),
            chain_depth: 0,
            emitting_trigger_id: None,
            event_hook: None,
        }
    );
    assert_eq!(
        map.get(ENV_SOURCE_THREAD_ID).map(String::as_str),
        Some(tid.to_string().as_str())
    );
}

#[test]
fn subprocess_origin_env_vars_without_a_thread_binds_the_sentinel() {
    install_test_secret();
    let vars = subprocess_origin_env_vars(None, 0, None);
    let map: std::collections::HashMap<_, _> = vars.iter().cloned().collect();
    assert!(vars.iter().all(|(k, _)| *k != ENV_SOURCE_THREAD_ID));

    let token = map
        .get(ENV_AGENT_ORIGIN_TOKEN)
        .expect("a thread-less spawn still gets a token");
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, token)]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess {
            source_thread_id: None,
            chain_depth: 0,
            emitting_trigger_id: None,
            event_hook: None,
        },
        "a scheduled script authenticates as a subprocess with no thread"
    );
}

#[test]
fn a_scheduled_script_token_carries_the_trigger_that_ran_it() {
    let trigger = Uuid::new_v4().to_string();
    let token = token_for_fire(None, Some(&trigger));
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess {
            source_thread_id: None,
            chain_depth: 0,
            emitting_trigger_id: Some(trigger),
            event_hook: None,
        },
        "the script IS the fire, so its emits must not wake the trigger"
    );
}

#[test]
fn a_tool_token_carries_both_the_thread_and_the_trigger() {
    let tid = Uuid::new_v4();
    let trigger = Uuid::new_v4().to_string();
    let token = token_for_fire(Some(tid), Some(&trigger));
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess {
            source_thread_id: Some(tid),
            chain_depth: 0,
            emitting_trigger_id: Some(trigger),
            event_hook: None,
        }
    );
}

#[test]
fn a_trigger_id_holding_a_separator_still_round_trips() {
    for id in ["scan.inbox.v2", "scan@inbox", "scan@v2.1"] {
        let token = token_for_fire(None, Some(id));
        let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
        assert_eq!(
            subprocess_origin(&h),
            SubprocessOrigin::Subprocess {
                source_thread_id: None,
                chain_depth: 0,
                emitting_trigger_id: Some(id.to_string()),
                event_hook: None,
            },
            "'{id}' must round-trip intact"
        );
    }
}

#[test]
fn a_trigger_id_that_is_not_header_safe_drops_only_the_claim() {
    let token = token_for_fire(None, Some("with a space"));
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
    assert_eq!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess {
            source_thread_id: None,
            chain_depth: 0,
            emitting_trigger_id: None,
            event_hook: None,
        }
    );
}

#[test]
fn a_trigger_claim_can_be_neither_forged_nor_stripped() {
    let tid = Uuid::new_v4();
    let victim = Uuid::new_v4().to_string();
    let other = Uuid::new_v4().to_string();
    let plain = token_for(Some(tid));
    let claimed = token_for_fire(Some(tid), Some(&victim));
    let plain_mac = plain.rsplit_once('.').expect("minted token has a mac").1;
    let claimed_mac = claimed.rsplit_once('.').expect("minted token has a mac").1;

    for (label, candidate) in [
        ("forged claim", format!("{tid}@0@-@{victim}.{plain_mac}")),
        ("stripped claim", format!("{tid}@0@-@-.{claimed_mac}")),
        ("swapped claim", format!("{tid}@0@-@{other}.{claimed_mac}")),
        ("dropped field", format!("{tid}@0.{claimed_mac}")),
    ] {
        let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &candidate)]);
        assert_eq!(
            subprocess_origin(&h),
            SubprocessOrigin::NotSubprocess,
            "{label} must not authenticate"
        );
    }
}

#[test]
fn a_caller_with_no_token_claims_no_trigger() {
    let trigger = Uuid::new_v4().to_string();
    let minted = token_for_fire(None, Some(&trigger));
    let signed = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &minted)]);
    assert_eq!(
        subprocess_origin(&signed),
        SubprocessOrigin::Subprocess {
            source_thread_id: None,
            chain_depth: 0,
            emitting_trigger_id: Some(trigger.clone()),
            event_hook: None,
        },
        "a minted token carries the fire it was minted for"
    );

    for (label, headers) in [
        (
            "an app through the SDK",
            headers_with(&[("user-agent", "Mozilla/5.0")]),
        ),
        (
            "a caller naming the trigger in the header",
            headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &trigger)]),
        ),
    ] {
        assert_eq!(
            subprocess_origin(&headers),
            SubprocessOrigin::NotSubprocess,
            "{label} must express no claim"
        );
    }
}

static API_PORT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn host_pid_var<'a>(vars: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
    vars.iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.as_str())
}

#[test]
fn host_protection_env_vars_always_sets_host_pid() {
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    let vars = host_protection_env_vars();
    assert_eq!(
        host_pid_var(&vars, "LUCIDOS_HOST_PID"),
        Some(std::process::id().to_string().as_str()),
        "LUCIDOS_HOST_PID must equal the engine's own pid"
    );
}

#[test]
fn the_build_watch_pid_is_read_only_while_its_lock_is_held() {
    use lucidos_owner_lock::{build_watch, OwnerLock};
    let app = tempfile::tempdir().expect("tempdir");
    let dist = app.path().join("dist");
    assert_eq!(build_watch_pid(&dist), None, "no watch has ever run here");

    let watcher = OwnerLock::acquire(&build_watch::watch_lock_path(app.path())).unwrap();
    watcher
        .publish_pid(&build_watch::pid_path(app.path()))
        .unwrap();
    assert_eq!(build_watch_pid(&dist), Some(std::process::id()));

    drop(watcher);
    assert_eq!(
        build_watch_pid(&dist),
        None,
        "the pidfile outlived its holder"
    );
}

#[test]
fn host_protection_env_vars_includes_api_port_when_engine_has_one() {
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    // SAFETY: process-wide env mutation gated by API_PORT_ENV_LOCK.
    unsafe {
        std::env::set_var("LUCIDOS_API_PORT", "3007");
    }
    let vars = host_protection_env_vars();
    unsafe {
        std::env::remove_var("LUCIDOS_API_PORT");
    }
    assert_eq!(host_pid_var(&vars, "LUCIDOS_API_PORT"), Some("3007"),);
}

#[test]
fn host_protection_env_vars_omits_api_port_when_engine_has_none() {
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::remove_var("LUCIDOS_API_PORT");
    }
    let vars = host_protection_env_vars();
    assert!(
        host_pid_var(&vars, "LUCIDOS_API_PORT").is_none(),
        "LUCIDOS_API_PORT must be unset when engine has no port (Tauri / tests)"
    );
}

#[test]
fn host_protection_env_vars_adds_loopback_base_url_under_gateway() {
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    // SAFETY: process-wide env mutation gated by API_PORT_ENV_LOCK.
    unsafe {
        std::env::set_var("LUCIDOS_API_PORT", "62072");
        std::env::set_var("LUCIDOS_BIND_LOOPBACK", "1");
    }
    let vars = host_protection_env_vars();
    unsafe {
        std::env::remove_var("LUCIDOS_API_PORT");
        std::env::remove_var("LUCIDOS_BIND_LOOPBACK");
    }
    let expected = format!("{}://127.0.0.1:62072", crate::net_config::tls_scheme());
    assert_eq!(
        host_pid_var(&vars, "LUCIDOS_API_BASE_URL"),
        Some(expected.as_str()),
        "loopback engine base URL must be handed to subprocesses under the gateway"
    );
}

#[test]
fn host_protection_env_vars_omits_base_url_without_gateway() {
    let _guard = API_PORT_ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("LUCIDOS_API_PORT", "5173");
        std::env::remove_var("LUCIDOS_BIND_LOOPBACK");
    }
    let vars = host_protection_env_vars();
    unsafe {
        std::env::remove_var("LUCIDOS_API_PORT");
    }
    assert!(
        host_pid_var(&vars, "LUCIDOS_API_BASE_URL").is_none(),
        "no base-URL override outside the gateway (the ports file resolves the engine)"
    );
}

#[test]
fn build_origin_subprocess_overrides_human_to_agent_api() {
    let source = Uuid::new_v4();
    let token = token_for(Some(source));
    let h = headers_with(&[
        ("user-agent", "curl/8.7.1"),
        (HEADER_AGENT_ORIGIN_TOKEN, &token),
    ]);
    let origin = build_message_origin(&h, ActorMode::Human, None, None, None, None, None);
    match origin {
        Some(MessageOrigin::Api {
            user_agent,
            mode,
            source_thread_id,
            ..
        }) => {
            assert_eq!(user_agent.as_deref(), Some("curl/8.7.1"));
            assert_eq!(
                mode,
                ActorMode::Agent,
                "subprocess origin must downgrade Human → Agent so the UI \
                 doesn't render an agent action as a 'You' card"
            );
            assert_eq!(
                source_thread_id,
                Some(source),
                "spawning thread id must flow through for popover attribution"
            );
        }
        other => panic!(
            "expected Api{{mode: Agent, source_thread_id: Some(_)}}, got {:?}",
            other
        ),
    }
}

#[test]
fn build_origin_subprocess_overrides_device_id_too() {
    let token = token_for(Some(Uuid::new_v4()));
    let h = headers_with(&[
        ("user-agent", "curl/8.7.1"),
        (HEADER_AGENT_ORIGIN_TOKEN, &token),
        ("x-lucidos-device-id", "should-be-ignored"),
    ]);
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("should-be-ignored"),
        None,
        None,
        None,
        None,
    );
    assert!(
        matches!(
            origin,
            Some(MessageOrigin::Api {
                mode: ActorMode::Agent,
                ..
            })
        ),
        "subprocess origin must beat a Device id, got {:?}",
        origin
    );
}

#[test]
fn build_origin_a_verified_token_outranks_the_caller_claim() {
    let token = token_for_fire(None, Some("trig-script"));
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
    let forged = Uuid::new_v4();
    let origin = build_message_origin(
        &h,
        ActorMode::Agent,
        None,
        None,
        None,
        None,
        caller("dev", Some(forged), None, ActorMode::Agent),
    );
    assert_eq!(
        origin,
        Some(MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Agent,
            source_thread_id: None,
            emitting_trigger_id: Some("trig-script".into()),
        })
    );
}

#[tokio::test]
async fn user_actor_subprocess_token_overrides_the_device() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    crate::test_support::seed_device(&pool, "real-device", Some("Mozilla"), Some("My MacBook"))
        .await;
    let source = Uuid::new_v4();
    let token = token_for(Some(source));
    let h = headers_with(&[
        ("user-agent", "curl/8.7.1"),
        (HEADER_DEVICE_ID, "real-device"),
        (HEADER_AGENT_ORIGIN_TOKEN, &token),
    ]);
    let actor = user_actor(&h, None);
    match actor {
        Some(MessageOrigin::Api {
            mode,
            source_thread_id,
            ..
        }) => {
            assert_eq!(mode, ActorMode::Agent);
            assert_eq!(source_thread_id, Some(source));
        }
        other => panic!(
            "subprocess token must beat resolved Device row, got {:?}",
            other
        ),
    }
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn owner_gate_refusals_name_the_act_and_the_next_step() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    init_agent_origin_secret("owner-device-gate-secret".into());
    let token = mint_agent_origin_token(Some(Uuid::new_v4()), 0, None).unwrap();
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "owner-app", Some("Mozilla/5.0"), None)
        .await
        .unwrap();
    let mut app = stamped_device_headers("owner-app");
    app.insert("referer", "https://localhost/app/example/".parse().unwrap());
    let act = "start the trigger grant review";

    for (headers, reason, guidance) in [
        (
            app,
            "owner_gate_app_document",
            "Use the control in the Lucidos UI",
        ),
        (
            headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]),
            "owner_gate_not_a_person",
            "If the owner asked you",
        ),
        (
            headers_with(&[(HEADER_DEVICE_ID, "owner-app")]),
            "not_attested_as_owner",
            "use a trigger",
        ),
        (
            stamped_device_headers("unconfirmed"),
            "owner_gate_device_unconfirmed",
            "Try again in a moment",
        ),
    ] {
        if reason == "owner_gate_device_unconfirmed" {
            pool.close().await;
        }
        let refusal = require_owner_device(&headers, &pool, &bus, None, act)
            .await
            .expect_err("the owner gate must refuse this caller");
        assert_eq!(refusal.status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(refusal.reason, Some(reason));
        if reason == "not_attested_as_owner" {
            assert_eq!(refusal.message, NOT_ATTESTED_AS_OWNER);
        } else {
            assert!(refusal.message.contains(act), "{}", refusal.message);
        }
        assert!(refusal.message.contains(guidance), "{}", refusal.message);
        assert!(!refusal.message.contains("Deleting a thread"));
    }

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_owners_registered_device_may_delete() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "owner-phone", Some("Mozilla/5.0"), None)
        .await
        .unwrap();

    let h = stamped_device_headers("owner-phone");
    let actor = require_owner_device(&h, &pool, &bus, None, "delete a thread")
        .await
        .expect("the owner's own device is exactly who this is for");
    assert!(matches!(actor, MessageOrigin::Device { .. }));

    let typed = headers_with(&[(HEADER_DEVICE_ID, "owner-phone")]);
    let refusal = require_owner_device(&typed, &pool, &bus, None, "delete a thread")
        .await
        .expect_err("a typed device id is not the owner's device");
    assert_eq!(refusal.status, axum::http::StatusCode::FORBIDDEN);

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_verified_agent_origin_token_may_not_delete() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    init_agent_origin_secret("owner-device-gate-secret".into());
    let token = mint_agent_origin_token(Some(Uuid::new_v4()), 0, None)
        .expect("the secret is installed, so a token mints");

    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token)]);
    assert!(matches!(
        subprocess_origin(&h),
        SubprocessOrigin::Subprocess { .. }
    ));

    let refusal = require_owner_device(&h, &pool, &bus, None, "delete a thread")
        .await
        .expect_err("no agent may press this button");
    assert_eq!(refusal.status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(refusal.reason, Some("owner_gate_not_a_person"));

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_agent_token_beside_a_real_device_header_is_still_refused() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "owner-laptop", Some("Mozilla/5.0"), None)
        .await
        .unwrap();
    init_agent_origin_secret("owner-device-gate-secret".into());
    let token = mint_agent_origin_token(Some(Uuid::new_v4()), 0, None).unwrap();

    let h = headers_with(&[
        (HEADER_DEVICE_ID, "owner-laptop"),
        (HEADER_AGENT_ORIGIN_TOKEN, &token),
    ]);
    let refusal = require_owner_device(&h, &pool, &bus, None, "delete a thread")
        .await
        .expect_err("borrowing the owner's device id does not make an agent a person");
    assert_eq!(refusal.status, axum::http::StatusCode::FORBIDDEN);

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_machine_local_token_may_not_delete() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let token = crate::api::local_auth::publish_test_local_token();
    let h = headers_with(&[(lucidos_local_token::HEADER_LOCAL_TOKEN, token)]);

    let refusal = require_owner_device(&h, &pool, &bus, None, "delete a thread")
        .await
        .expect_err("the engine's own machinery is not the owner");
    assert_eq!(refusal.status, axum::http::StatusCode::FORBIDDEN);

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_unattributed_caller_may_not_delete() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let h = headers_with(&[("user-agent", "curl/8")]);

    let refusal = require_owner_device(&h, &pool, &bus, None, "delete a thread")
        .await
        .expect_err("a bare curl is nobody");
    assert_eq!(refusal.status, axum::http::StatusCode::UNAUTHORIZED);

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_unregistered_device_may_not_delete() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let h = headers_with(&[(HEADER_DEVICE_ID, "never-registered")]);

    let refusal = require_owner_device(&h, &pool, &bus, None, "delete a thread")
        .await
        .expect_err("an id naming no device is not the owner");
    assert_eq!(refusal.status, axum::http::StatusCode::UNAUTHORIZED);
    assert!(
        !crate::core::DeviceStore::is_registered(&pool, "never-registered")
            .await
            .unwrap(),
        "only a stamped device is registered, never a typed id"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_database_blip_cannot_promote_an_invented_device() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let h = stamped_device_headers("invented");

    pool.close().await;
    assert!(matches!(
        require_user_actor(&h, &pool, None).await,
        Ok(MessageOrigin::Device { .. })
    ));

    let refusal = require_owner_device(&h, &pool, &bus, None, "delete a thread")
        .await
        .expect_err("an unconfirmable device must not delete a thread");
    assert_eq!(refusal.status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(refusal.reason, Some("owner_gate_device_unconfirmed"));

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_stamped_device_without_a_row_is_registered_and_admitted() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let mut h = stamped_device_headers("re-paired-window");
    h.insert("user-agent", "Mozilla/5.0".parse().unwrap());

    for _ in 0..2 {
        let actor = require_owner_device(&h, &pool, &bus, None, "delete a thread")
            .await
            .expect("a stamped device is admitted without a client round trip");
        assert!(matches!(actor, MessageOrigin::Device { .. }));
    }

    assert!(
        crate::core::DeviceStore::is_registered(&pool, "re-paired-window")
            .await
            .unwrap(),
        "the gate registers the stamped device"
    );
    let announced: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'DeviceRegistered'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(announced, 1, "DeviceRegistered fires once, not per request");

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_hand_over_after_the_gate_registered_merges_the_old_row() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "old-window", Some("UA"), None)
        .await
        .unwrap();
    let h = stamped_device_headers("new-window");
    require_owner_device(&h, &pool, &bus, None, "delete a thread")
        .await
        .unwrap();

    let outcome =
        crate::core::DeviceStore::hand_over(&pool, &bus, "old-window", "new-window", None)
            .await
            .expect("the hand over reads and writes");
    assert_eq!(outcome, crate::core::devices::HandOver::Moved);
    assert!(
        !crate::core::DeviceStore::is_registered(&pool, "old-window")
            .await
            .unwrap()
    );
    assert!(crate::core::DeviceStore::is_registered(&pool, "new-window")
        .await
        .unwrap());

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_app_document_may_not_delete() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "owner-tablet", Some("Mozilla/5.0"), None)
        .await
        .unwrap();

    let mut h = headers_with(&[
        ("sec-fetch-site", "same-origin"),
        ("referer", "https://localhost:5173/app/habit-tracker/"),
    ]);
    insert_stamped_device(&mut h, "owner-tablet");
    let refusal = require_owner_device(&h, &pool, &bus, None, "delete a thread")
        .await
        .expect_err("an app must not press the owner's button with the owner's id");
    assert_eq!(refusal.status, axum::http::StatusCode::FORBIDDEN);

    let mut unregistered = headers_with(&[
        ("sec-fetch-site", "same-origin"),
        ("referer", "https://localhost:5173/app/habit-tracker/"),
    ]);
    insert_stamped_device(&mut unregistered, "app-named-window");
    require_owner_device(&unregistered, &pool, &bus, None, "delete a thread")
        .await
        .expect_err("an app document registers no device");
    assert!(
        !crate::core::DeviceStore::is_registered(&pool, "app-named-window")
            .await
            .unwrap()
    );

    let mut shell = headers_with(&[
        ("sec-fetch-site", "same-origin"),
        ("referer", "https://localhost:5173/"),
    ]);
    insert_stamped_device(&mut shell, "owner-tablet");
    assert!(
        require_owner_device(&shell, &pool, &bus, None, "delete a thread",)
            .await
            .is_ok()
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[test]
fn a_verified_origin_token_is_refused_as_an_agent() {
    let h = headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, &token_for(Some(Uuid::new_v4())))]);
    let refusal = require_attested_owner(&h, "only the owner sends this").unwrap_err();
    assert_eq!(refusal.status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(refusal.message, "only the owner sends this");
}

#[test]
fn a_stamped_device_is_the_attested_owner() {
    let local = crate::api::local_auth::publish_test_local_token();
    let mut h = headers_with(&[(lucidos_local_token::HEADER_LOCAL_TOKEN, local)]);
    insert_stamped_device(&mut h, "device-abc");
    assert!(require_attested_owner(&h, "refused",).is_ok());
    assert!(require_attested_owner(&stamped_device_headers("device-abc"), "refused",).is_ok());
}

#[test]
fn an_unattested_caller_is_refused_as_not_the_owner() {
    let local = crate::api::local_auth::publish_test_local_token();
    install_test_secret();
    for h in [
        headers_with(&[(HEADER_DEVICE_ID, "device-abc")]),
        headers_with(&[(lucidos_local_token::HEADER_LOCAL_TOKEN, local)]),
        headers_with(&[
            (HEADER_DEVICE_ID, "device-abc"),
            (lucidos_local_token::HEADER_LOCAL_TOKEN, local),
        ]),
        headers_with(&[(HEADER_AGENT_ORIGIN_TOKEN, "not-a-token")]),
        headers_with(&[]),
    ] {
        let refusal = require_attested_owner(&h, "agent refusal").unwrap_err();
        assert_eq!(refusal.status, axum::http::StatusCode::FORBIDDEN, "{h:?}");
        assert_eq!(refusal.message, NOT_ATTESTED_AS_OWNER, "{h:?}");
    }
}

#[tokio::test]
async fn snapshot_policy_write_requires_registered_owner_device() {
    let (pool, name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    crate::core::DeviceStore::register(&pool, &bus, "owner-phone", None, None)
        .await
        .unwrap();
    let key = crate::core::prefs::SNAPSHOT_RESTORE_BY_AGENTS.key();
    let local = crate::api::local_auth::publish_test_local_token();
    install_test_secret();
    let token = mint_agent_origin_token(Some(Uuid::new_v4()), 0, None).unwrap();
    for headers in [
        headers_with(&[]),
        headers_with(&[(HEADER_DEVICE_ID, "unknown")]),
        headers_with(&[(HEADER_DEVICE_ID, "owner-phone")]),
        headers_with(&[(lucidos_local_token::HEADER_LOCAL_TOKEN, local)]),
        headers_with(&[
            (HEADER_AGENT_ORIGIN_TOKEN, &token),
            (HEADER_DEVICE_ID, "owner-phone"),
        ]),
    ] {
        let refusal =
            crate::api::settings::preference_write_actor(&headers, &pool, &bus, key, None)
                .await
                .unwrap_err();
        assert!(matches!(
            refusal.status,
            axum::http::StatusCode::FORBIDDEN | axum::http::StatusCode::UNAUTHORIZED
        ));
        if refusal.status == axum::http::StatusCode::FORBIDDEN {
            if refusal.reason == Some("owner_gate_not_a_person") {
                assert!(refusal.message.contains("change who can restore snapshots"));
            } else {
                assert_eq!(refusal.message, NOT_ATTESTED_AS_OWNER);
            }
        }
    }
    let owner = stamped_device_headers("owner-phone");
    assert!(matches!(
        crate::api::settings::preference_write_actor(&owner, &pool, &bus, key, None)
            .await
            .unwrap(),
        Some(MessageOrigin::Device { .. })
    ));
    assert!(crate::api::settings::preference_write_actor(
        &headers_with(&[]),
        &pool,
        &bus,
        "theme",
        None
    )
    .await
    .is_ok());
    crate::test_support::teardown_test_db(&name).await;
}

#[test]
fn a_device_session_builds_the_standard_device_origin() {
    let h = crate::api::gateway_stamp::test_support::session_device_headers("direct-device");
    let origin = build_message_origin(
        &h,
        ActorMode::Human,
        Some("direct-device"),
        None,
        None,
        None,
        None,
    );
    assert_eq!(
        serde_json::to_value(origin).unwrap(),
        serde_json::json!({ "kind": "device", "device_id": "direct-device" })
    );
}

#[tokio::test]
async fn a_session_device_can_resolve_its_targeted_fit_answer() {
    use crate::engine::pending_answers::{await_answer, resolve_answer, PendingAnswers};
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let h = crate::api::gateway_stamp::test_support::session_device_headers("direct-device");
    let actor = require_user_actor(&h, &pool, Some("direct-device"))
        .await
        .unwrap();
    let MessageOrigin::Device { device_id } = actor else {
        panic!("expected the session device");
    };
    let pending: PendingAnswers<Vec<u8>> = Default::default();
    let answer = await_answer(&pending, "fit-check", Some("direct-device"));
    assert_eq!(
        resolve_answer(&pending, "fit-check", Some(&device_id), vec![1]),
        Ok(())
    );
    assert_eq!(answer.await.unwrap(), vec![1]);
    crate::test_support::teardown_test_db(&db_name).await;
}
