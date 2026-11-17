use super::*;
use crate::api::permission_requests::tests::{agent_headers, agent_thread, grant};
use crate::engine::permission::menu::{ALLOW_FOR_THIS_THREAD_LABEL, ALLOW_ONCE_LABEL};
use crate::test_support::{setup_test_db, teardown_test_db};
use std::sync::{Arc, Mutex};

/// A gateway listing `demo` (machine-made) and `home` (a user's), recording
/// every POST path it receives.
async fn stand_in_gateway() -> (u16, Arc<Mutex<Vec<String>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let app = axum::Router::<()>::new()
        .route(
            "/~/api/v1/control/workspaces",
            get(|| async {
                Json(serde_json::json!({ "workspaces": [
                    {"id": "demo", "name": "Demo", "path": "/home/user/workspaces/demo",
                     "port": 5200, "health": "healthy", "autostart": false,
                     "origin": "demo", "machine_made": true},
                    {"id": "home", "name": "Home", "path": "/home/user/workspaces/home",
                     "port": 5201, "health": "healthy", "autostart": true,
                     "origin": "user", "machine_made": false},
                ]}))
            }),
        )
        .fallback(
            move |uri: axum::http::Uri, Json(body): Json<serde_json::Value>| {
                let seen = seen.clone();
                async move {
                    if body["check_only"] == true {
                        return Json(serde_json::json!({"checked": true})).into_response();
                    }
                    seen.lock().unwrap().push(uri.path().to_owned());
                    if uri.path().ends_with("/device-session") {
                        Json(serde_json::json!({"session": "fixture-session", "expires_at": 12345}))
                            .into_response()
                    } else {
                        StatusCode::NO_CONTENT.into_response()
                    }
                }
            },
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, calls)
}

async fn call(
    bus: &EventBus,
    port: u16,
    headers: &HeaderMap,
    verb: Action,
    target: &str,
) -> Result<Response, ApiError> {
    let broker = Broker {
        bus,
        port: port.to_string(),
        own: "rig".into(),
    };
    let call = Call {
        verb,
        target,
        reason: "The rig needs it.",
    };
    broker.on_listed(headers, call, serde_json::json!({})).await
}

async fn body_json(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn requests_recorded(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'PermissionRequestPrepared'")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Grant `label` on a request for `verb` at `target`, as the owner would.
async fn allow(bus: &EventBus, caller: Uuid, verb: Action, target: &str, label: &str) {
    let ask = workspace_ask(verb, target, "I need it.").unwrap();
    grant(bus, caller, ask, label).await;
}

/// Invariants 1 and 2 end to end, on a user's workspace: with no permission
/// the call is refused and the gateway is not called. Allow once acts once,
/// then the call asks again.
#[tokio::test]
async fn a_stop_asks_first_then_acts_once_on_the_allow() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (port, calls) = stand_in_gateway().await;
    let caller = agent_thread(&bus).await;
    let headers = agent_headers(Some(caller));

    let asked = call(&bus, port, &headers, Action::StopWorkspace, "home")
        .await
        .unwrap();
    assert_eq!(asked.status(), StatusCode::FORBIDDEN);
    let body = body_json(asked).await;
    assert_eq!(body["reason"], PERMISSION_REQUIRED);
    assert!(body["request_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("permission-request:")));
    assert!(calls.lock().unwrap().is_empty());

    allow(
        &bus,
        caller,
        Action::StopWorkspace,
        "home",
        ALLOW_ONCE_LABEL,
    )
    .await;
    let done = call(&bus, port, &headers, Action::StopWorkspace, "home")
        .await
        .unwrap();
    assert_eq!(done.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        *calls.lock().unwrap(),
        ["/~/api/v1/control/workspaces/home/stop"]
    );

    let again = call(&bus, port, &headers, Action::StopWorkspace, "home")
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::FORBIDDEN);
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert!(agents_acts_recorded(&pool).await.is_empty());
    pool.close().await;
    teardown_test_db(&db).await;
}

/// Invariant 2: two racing calls on one Allow once act once between them.
#[tokio::test]
async fn two_racing_stops_spend_one_allow_once_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (port, calls) = stand_in_gateway().await;
    let caller = agent_thread(&bus).await;
    let headers = agent_headers(Some(caller));
    allow(
        &bus,
        caller,
        Action::StopWorkspace,
        "home",
        ALLOW_ONCE_LABEL,
    )
    .await;

    let (a, b) = tokio::join!(
        call(&bus, port, &headers, Action::StopWorkspace, "home"),
        call(&bus, port, &headers, Action::StopWorkspace, "home"),
    );
    let mut statuses = [a.unwrap().status(), b.unwrap().status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::NO_CONTENT, StatusCode::FORBIDDEN]);
    assert_eq!(calls.lock().unwrap().len(), 1);
    pool.close().await;
    teardown_test_db(&db).await;
}

/// Allow for this thread lets the asking thread act again and again on that
/// workspace, and no other thread.
#[tokio::test]
async fn allow_for_this_thread_acts_again_on_its_own_thread_only() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (port, calls) = stand_in_gateway().await;
    let caller = agent_thread(&bus).await;
    let headers = agent_headers(Some(caller));
    allow(
        &bus,
        caller,
        Action::StartWorkspace,
        "home",
        ALLOW_FOR_THIS_THREAD_LABEL,
    )
    .await;

    for _ in 0..2 {
        let done = call(&bus, port, &headers, Action::StartWorkspace, "home")
            .await
            .unwrap();
        assert_eq!(done.status(), StatusCode::NO_CONTENT);
    }
    let other = agent_headers(Some(agent_thread(&bus).await));
    let asked = call(&bus, port, &other, Action::StartWorkspace, "home")
        .await
        .unwrap();
    assert_eq!(asked.status(), StatusCode::FORBIDDEN);
    let stop = call(&bus, port, &headers, Action::StopWorkspace, "home")
        .await
        .unwrap();
    assert_eq!(stop.status(), StatusCode::FORBIDDEN, "it covers one verb");
    assert_eq!(calls.lock().unwrap().len(), 2);
    pool.close().await;
    teardown_test_db(&db).await;
}

async fn agents_acts_recorded(pool: &sqlx::PgPool) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT payload FROM events WHERE event_type = 'MachineMadeWorkspaceChanged' \
         ORDER BY sequence",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A machine-made workspace is the agent's: stop and start act with no card,
/// and each records one `MachineMadeWorkspaceChanged`.
#[tokio::test]
async fn a_machine_made_workspace_stops_and_starts_with_no_card() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (port, calls) = stand_in_gateway().await;
    let headers = agent_headers(Some(Uuid::new_v4()));

    for verb in [Action::StopWorkspace, Action::StartWorkspace] {
        let done = call(&bus, port, &headers, verb, "demo").await.unwrap();
        assert_eq!(done.status(), StatusCode::NO_CONTENT, "{verb:?}");
    }
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "/~/api/v1/control/workspaces/demo/stop",
            "/~/api/v1/control/workspaces/demo/start"
        ]
    );
    assert_eq!(requests_recorded(&pool).await, 0);
    let recorded = agents_acts_recorded(&pool).await;
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0]["workspace_id"], "demo");
    assert_eq!(recorded[0]["reason"], "The rig needs it.");
    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn only_reversible_verbs_on_a_machine_made_workspace_skip_the_card() {
    let agents = [
        Action::StopWorkspace,
        Action::StartWorkspace,
        Action::SetWorkspaceAutostart,
        Action::RenameWorkspace,
        Action::RestoreSnapshot,
    ];
    let users = [
        Action::CreateWorkspace,
        Action::DeleteWorkspace,
        Action::RestoreWorkspace,
    ];
    for verb in agents {
        assert_eq!(whose(verb, true), Whose::Agents, "{verb:?}");
        assert_eq!(whose(verb, false), Whose::Users, "{verb:?}");
    }
    for verb in users {
        assert_eq!(whose(verb, true), Whose::Users, "{verb:?}");
        assert_eq!(whose(verb, false), Whose::Users, "{verb:?}");
    }
}

/// I2: a user workspace's delete is refused before any card or gateway call.
#[tokio::test]
async fn a_user_workspace_delete_is_refused_before_a_card() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (port, calls) = stand_in_gateway().await;
    let caller = agent_thread(&bus).await;
    let headers = agent_headers(Some(caller));

    let refused = call(&bus, port, &headers, Action::DeleteWorkspace, "home")
        .await
        .unwrap_err();
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(
        refused.message.contains("machine-made"),
        "{}",
        refused.message
    );
    assert_eq!(requests_recorded(&pool).await, 0);

    let missing = call(&bus, port, &headers, Action::StartWorkspace, "nowhere")
        .await
        .unwrap_err();
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    allow(
        &bus,
        caller,
        Action::DeleteWorkspace,
        "demo",
        ALLOW_ONCE_LABEL,
    )
    .await;
    let done = call(&bus, port, &headers, Action::DeleteWorkspace, "demo")
        .await
        .unwrap();
    assert_eq!(done.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        *calls.lock().unwrap(),
        ["/~/api/v1/control/broker/workspaces/demo/delete"]
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

/// An agent never stops or deletes the engine it runs in: that would end its
/// own turn mid-call.
#[tokio::test]
async fn an_agent_cannot_stop_or_delete_its_own_workspace() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (port, calls) = stand_in_gateway().await;
    let headers = agent_headers(Some(Uuid::new_v4()));
    for verb in [Action::StopWorkspace, Action::DeleteWorkspace] {
        let refused = call(&bus, port, &headers, verb, "rig").await.unwrap_err();
        assert_eq!(refused.status, StatusCode::CONFLICT);
    }
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(requests_recorded(&pool).await, 0);
    pool.close().await;
    teardown_test_db(&db).await;
}

/// A rename to a name another workspace carries is refused before any card,
/// matched the gateway's way: trimmed and case-insensitive.
#[tokio::test]
async fn a_rename_to_a_taken_name_is_refused_before_a_card() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (port, calls) = stand_in_gateway().await;
    let headers = agent_headers(Some(Uuid::new_v4()));
    let broker = Broker {
        bus: &bus,
        port: port.to_string(),
        own: "rig".into(),
    };
    let rename = |target| Call {
        verb: Action::RenameWorkspace,
        target,
        reason: "The rig needs it.",
    };
    for (target, taken) in [("demo", " home "), ("home", "DEMO")] {
        let refused = broker
            .on_listed(
                &headers,
                rename(target),
                serde_json::json!({ "name": taken }),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.status, StatusCode::CONFLICT, "{target}");
    }
    assert_eq!(requests_recorded(&pool).await, 0);
    assert!(agents_acts_recorded(&pool).await.is_empty());

    let done = broker
        .on_listed(
            &headers,
            rename("demo"),
            serde_json::json!({ "name": "DEMO" }),
        )
        .await
        .unwrap();
    assert_eq!(
        done.status(),
        StatusCode::NO_CONTENT,
        "its own name is free"
    );
    assert_eq!(
        *calls.lock().unwrap(),
        ["/~/api/v1/control/workspaces/demo/rename"]
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn a_create_name_matches_ignoring_case_and_spaces() {
    let listing: Vec<ListedWorkspace> = serde_json::from_value(serde_json::json!([
        {"id": "demo-rig", "name": "Demo Rig", "path": "/home/user/workspaces/demo-rig",
         "port": 5300, "health": "healthy", "autostart": false},
    ]))
    .unwrap();
    assert!(refuse_a_taken_name(&listing, "  demo rig ", None).is_err());
    assert!(refuse_a_taken_name(&listing, "Demo Rig", Some("demo-rig")).is_ok());
    assert!(refuse_a_taken_name(&listing, "Demo Rig 2", None).is_ok());
}

/// D3: a caller with no thread-bound origin token is refused.
#[tokio::test]
async fn only_an_agent_thread_calls_the_broker() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (port, calls) = stand_in_gateway().await;
    for headers in [HeaderMap::new(), agent_headers(None)] {
        let refused = call(&bus, port, &headers, Action::StopWorkspace, "demo")
            .await
            .unwrap_err();
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
    }
    assert!(calls.lock().unwrap().is_empty());
    pool.close().await;
    teardown_test_db(&db).await;
}

/// I3: restart is refused, and names stop then start.
#[tokio::test]
async fn restart_is_refused_with_stop_then_start() {
    let refusal = restart().await;
    assert_eq!(refusal.status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(refusal
        .message
        .contains("Stop the workspace, then start it"));
}

#[test]
fn a_trash_row_projects_only_its_named_fields() {
    let rows: Vec<TrashRow> = serde_json::from_str(
        r#"[{"id":"workspace-demo-1","name":"Demo","status":"completed",
             "restore_available":true,"machine_made":true,"bytes":10,"warning":null}]"#,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&rows).unwrap(),
        serde_json::json!([{"id": "workspace-demo-1", "name": "Demo", "status": "completed",
                            "restore_available": true, "machine_made": true}])
    );
}

/// I7: a classification skips once any named entry left `unknown` or is gone.
#[test]
fn a_classification_drifts_when_an_entry_is_classified_or_gone() {
    let listing: Vec<ListedWorkspace> = serde_json::from_value(serde_json::json!([
        {"id": "old-1", "name": "Old 1", "path": "/home/user/workspaces/old-1",
         "port": 5300, "health": "stopped", "autostart": false},
        {"id": "demo", "name": "Demo", "path": "/home/user/workspaces/demo",
         "port": 5301, "health": "stopped", "autostart": false, "origin": "demo"},
    ]))
    .unwrap();
    assert_eq!(classification_drift(&listing, &["old-1".into()]), None);
    assert!(
        classification_drift(&listing, &["old-1".into(), "demo".into()])
            .is_some_and(|why| why.contains("already classified"))
    );
    assert!(classification_drift(&listing, &["gone".into()])
        .is_some_and(|why| why.contains("no longer exists")));
}

#[tokio::test]
async fn device_sessions_spend_only_the_exact_owner_grant() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (port, calls) = stand_in_gateway().await;
    let caller = agent_thread(&bus).await;
    let headers = agent_headers(Some(caller));
    let broker = Broker {
        bus: &bus,
        port: port.to_string(),
        own: "rig".into(),
    };
    let body = |device: &str, ttl| DeviceSessionBody {
        device_id: device.into(),
        ttl_secs: Some(ttl),
        reason: "Drive the fixture page.".into(),
    };
    assert_eq!(whose(Action::GrantDeviceSession, true), Whose::Users);
    assert!(broker
        .grant_device_session(&headers, "home", body("page", 120))
        .await
        .is_err());
    assert_eq!(requests_recorded(&pool).await, 0);
    let asked = broker
        .grant_device_session(&headers, "demo", body("page", 120))
        .await
        .unwrap();
    assert_eq!(asked.status(), StatusCode::FORBIDDEN);
    assert!(calls.lock().unwrap().is_empty());
    let target =
        super::super::permission_requests::device_session_target("demo", "page", 120).unwrap();
    allow(
        &bus,
        caller,
        Action::GrantDeviceSession,
        &target,
        ALLOW_ONCE_LABEL,
    )
    .await;
    for (device, ttl) in [("other", 120), ("page", 121)] {
        let refused = broker
            .grant_device_session(&headers, "demo", body(device, ttl))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    }
    assert!(calls.lock().unwrap().is_empty());
    let done = broker
        .grant_device_session(&headers, "demo", body("page", 120))
        .await
        .unwrap();
    assert_eq!(done.status(), StatusCode::OK);
    assert_eq!(body_json(done).await["session"], "fixture-session");
    assert_eq!(calls.lock().unwrap().len(), 1);
    let again = broker
        .grant_device_session(&headers, "demo", body("page", 120))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::FORBIDDEN);
    assert_eq!(calls.lock().unwrap().len(), 1);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn marked_refusal_restores_once_but_unknown_outcomes_stay_spent() {
    for (status, marked) in [(409, true), (409, false), (500, false)] {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let caller = agent_thread(&bus).await;
        allow(
            &bus,
            caller,
            Action::StopWorkspace,
            "home",
            ALLOW_ONCE_LABEL,
        )
        .await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = attempts.clone();
        let app = Router::<()>::new().fallback(move || {
            let seen = seen.clone();
            async move {
                if seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    let mut body = serde_json::json!({"error": "fixture refusal"});
                    if marked {
                        body["changed"] = false.into();
                    }
                    (StatusCode::from_u16(status).unwrap(), Json(body)).into_response()
                } else {
                    StatusCode::NO_CONTENT.into_response()
                }
            }
        });
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let broker = Broker {
            bus: &bus,
            port: port.to_string(),
            own: "rig".into(),
        };
        let act = || {
            broker.act(
                caller,
                Call {
                    verb: Action::StopWorkspace,
                    target: "home",
                    reason: "fixture",
                },
                Whose::Users,
                "/workspaces/home/stop",
                serde_json::json!({}),
            )
        };
        assert!(act().await.is_err());
        let again = act().await.unwrap();
        assert_eq!(matches!(again, Outcome::Done(_)), marked);
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            if marked { 2 } else { 1 }
        );
        let restores: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type = 'PermissionRestored'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(restores, i64::from(marked));
        assert!(matches!(act().await.unwrap(), Outcome::NeedsPermission(_)));
        if marked {
            let spent: String = sqlx::query_scalar(
                "SELECT payload->>'spent' FROM events WHERE event_type = 'PermissionRestored'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(bus
                .emit(BusEvent::Thread {
                    thread_id: caller,
                    event: ThreadEvent::PermissionRestored {
                        spent: spent.parse().unwrap(),
                        restored: Uuid::new_v4(),
                    },
                    meta: EventMeta::NONE,
                })
                .await
                .is_err());
        }
        server.abort();
        pool.close().await;
        teardown_test_db(&db).await;
    }
}

#[tokio::test]
async fn device_session_precheck_refusal_prepares_no_request() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = agent_thread(&bus).await;
    let app = Router::<()>::new()
        .route(
            "/~/api/v1/control/workspaces",
            get(|| async {
                Json(serde_json::json!({"workspaces": [{
                    "id": "demo", "name": "Demo", "path": "/home/user/demo", "port": 5200,
                    "health": "stopped", "autostart": false, "origin": "demo", "machine_made": true
                }]}))
            }),
        )
        .fallback(|| async {
            (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "paired device", "changed": false, "reason": lucidos_gateway_constants::DEVICE_SESSION_OWNER_DEVICE})),
            )
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let broker = Broker {
        bus: &bus,
        port: port.to_string(),
        own: "rig".into(),
    };
    let refusal = broker
        .grant_device_session(
            &agent_headers(Some(caller)),
            "demo",
            DeviceSessionBody {
                device_id: "page".into(),
                ttl_secs: Some(120),
                reason: "fixture".into(),
            },
        )
        .await
        .expect_err("precheck must refuse");
    assert_eq!(
        refusal.reason,
        Some(lucidos_gateway_constants::DEVICE_SESSION_OWNER_DEVICE)
    );
    assert_eq!(requests_recorded(&pool).await, 0);
    server.abort();
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_gateway_timeout_leaves_the_once_grant_spent() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = agent_thread(&bus).await;
    allow(
        &bus,
        caller,
        Action::StopWorkspace,
        "home",
        ALLOW_ONCE_LABEL,
    )
    .await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let seen = reached.clone();
    let app = Router::<()>::new().fallback(move || {
        seen.notify_one();
        std::future::pending::<StatusCode>()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let owned_bus = bus.clone();
    let act = tokio::spawn(async move {
        let broker = Broker {
            bus: &owned_bus,
            port: port.to_string(),
            own: "rig".into(),
        };
        broker
            .act(
                caller,
                Call {
                    verb: Action::StopWorkspace,
                    target: "home",
                    reason: "fixture",
                },
                Whose::Users,
                "/workspaces/home/stop",
                serde_json::json!({}),
            )
            .await
    });
    reached.notified().await;
    tokio::time::pause();
    tokio::time::advance(BROKER_DEADLINE).await;
    assert!(act.await.unwrap().is_err());
    tokio::time::resume();
    assert!(crate::engine::permission::live_permission(
        &pool,
        &super::super::permission_requests::thread_user(caller),
        Action::StopWorkspace,
        Some("home")
    )
    .await
    .unwrap()
    .is_none());
    let restores: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'PermissionRestored'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(restores, 0);
    server.abort();
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn unchanged_refusals_restore_neither_lasting_grants_nor_agents_acts() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = agent_thread(&bus).await;
    allow(
        &bus,
        caller,
        Action::StartWorkspace,
        "home",
        ALLOW_FOR_THIS_THREAD_LABEL,
    )
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::<()>::new().fallback(|| async {
        (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "fixture", "changed": false})),
        )
    });
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let broker = Broker {
        bus: &bus,
        port: port.to_string(),
        own: "rig".into(),
    };
    for decider in [Whose::Users, Whose::Agents] {
        assert!(broker
            .act(
                caller,
                Call {
                    verb: Action::StartWorkspace,
                    target: "home",
                    reason: "fixture"
                },
                decider,
                "/workspaces/home/start",
                serde_json::json!({})
            )
            .await
            .is_err());
    }
    let restores: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'PermissionRestored'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(restores, 0);
    server.abort();
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn restoration_keeps_the_original_turn_boundary() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let caller = agent_thread(&bus).await;
    allow(
        &bus,
        caller,
        Action::StopWorkspace,
        "home",
        ALLOW_ONCE_LABEL,
    )
    .await;
    let user = super::super::permission_requests::thread_user(caller);
    let permission = crate::engine::permission::live_permission(
        &pool,
        &user,
        Action::StopWorkspace,
        Some("home"),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(crate::engine::permission::spend(&bus, &permission).await);
    super::super::permission_requests::tests::open_turn(
        &bus,
        caller,
        crate::engine::thread_events::MessageOrigin::Device {
            device_id: "fixture-phone".into(),
        },
    )
    .await;
    bus.emit(BusEvent::Thread {
        thread_id: caller,
        event: ThreadEvent::PermissionRestored {
            spent: permission.permission_id,
            restored: Uuid::new_v4(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert!(crate::engine::permission::live_permission(
        &pool,
        &user,
        Action::StopWorkspace,
        Some("home")
    )
    .await
    .unwrap()
    .is_none());
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn standing_device_session_consent_mints_successive_targets_and_revalidates() {
    use crate::api::permission_requests::tests::{ask_prepared, owner_device, tap};
    use crate::engine::permission::{menu::ALWAYS_ALLOW_LABEL, Resolution};
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mints = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let seen = mints.clone();
    let app = axum::Router::<()>::new().route("/~/api/v1/control/workspaces", get(|| async {
        let workspaces = ["take-one", "take-two", "legacy"].map(|id| serde_json::json!({
            "id": id, "name": id, "path": format!("/home/user/workspaces/{id}"),
            "port": 5200, "health": "healthy", "autostart": false, "origin": "agent", "machine_made": true
        }));
        Json(serde_json::json!({"workspaces": workspaces}))
    })).fallback(move |uri: axum::http::Uri, Json(body): Json<serde_json::Value>| {
        let seen = seen.clone();
        async move {
            if body["check_only"] == true {
                return Json(serde_json::json!({"checked": true,
                    "durable_eligible": !uri.path().contains("legacy") && body["caller"]["workspace_id"] == "source"})).into_response();
            }
            assert_eq!(body["creator_workspace"], "source");
            seen.lock().unwrap().push(body);
            Json(serde_json::json!({"session": "fixture-session", "expires_at": 12345})).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let broker = Broker {
        bus: &bus,
        port: port.to_string(),
        own: "source".into(),
    };
    let body = |device: &str, ttl| DeviceSessionBody {
        device_id: device.into(),
        ttl_secs: Some(ttl),
        reason: "Film fixtures.".into(),
    };
    let caller = agent_thread(&bus).await;
    let response = broker
        .grant_device_session(&agent_headers(Some(caller)), "take-one", body("page", 120))
        .await
        .unwrap();
    let response = body_json(response).await;
    let request_id = response["request_id"].as_str().unwrap();
    let card = ask_prepared(&bus, caller, request_id).await;
    let picked = card.pick(ALWAYS_ALLOW_LABEL);
    assert!(matches!(
        tap(&bus, caller, &card, picked, Some(owner_device())).await,
        Resolution::Granted(_)
    ));
    let other = agent_thread(&bus).await;
    let headers = agent_headers(Some(other));
    let before = requests_recorded(&pool).await;
    for target in ["take-one", "take-two"] {
        assert_eq!(
            broker
                .grant_device_session(&headers, target, body("page", 120))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(requests_recorded(&pool).await, before);
    for (target, device, ttl) in [
        ("legacy", "page", 120),
        ("take-two", "other", 120),
        ("take-two", "page", 119),
    ] {
        assert_eq!(
            broker
                .grant_device_session(&headers, target, body(device, ttl))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(mints.lock().unwrap().len(), 2);
    pool.close().await;
    teardown_test_db(&db).await;
}
