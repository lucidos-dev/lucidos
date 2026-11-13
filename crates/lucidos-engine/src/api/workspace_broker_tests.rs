use super::*;
use crate::api::owner_approval::tests::{agent_headers, allow_workspace_act};
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
        .fallback(move |uri: axum::http::Uri| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(uri.path().to_owned());
                StatusCode::NO_CONTENT
            }
        });
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
    sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'OwnerApprovalRequested'")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// I4 end to end, on a user's workspace: no card asks, an Allow acts once,
/// then it asks again.
#[tokio::test]
async fn a_stop_asks_first_then_acts_once_on_the_allow() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (port, calls) = stand_in_gateway().await;
    let caller = Uuid::new_v4();
    let headers = agent_headers(Some(caller));

    let asked = call(&bus, port, &headers, Action::StopWorkspace, "home")
        .await
        .unwrap();
    assert_eq!(asked.status(), StatusCode::FORBIDDEN);
    let body = body_json(asked).await;
    assert_eq!(body["reason"], OWNER_APPROVAL_REQUIRED);
    assert!(body["request_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("owner-approval:")));
    assert!(calls.lock().unwrap().is_empty());

    allow_workspace_act(&bus, caller, Action::StopWorkspace, "home").await;
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
    let caller = Uuid::new_v4();
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

    allow_workspace_act(&bus, caller, Action::DeleteWorkspace, "demo").await;
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
