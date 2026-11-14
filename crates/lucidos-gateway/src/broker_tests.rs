use super::*;
use crate::registry::{Workspace, WorkspaceOrigin};
use axum::body::Body;
use axum::http::{header, HeaderValue, Request};
use tower::ServiceExt as _;

const MACHINE_MADE: [WorkspaceOrigin; 4] = [
    WorkspaceOrigin::Agent,
    WorkspaceOrigin::Demo,
    WorkspaceOrigin::Eval,
    WorkspaceOrigin::E2e,
];
const PROTECTED: [WorkspaceOrigin; 2] = [WorkspaceOrigin::User, WorkspaceOrigin::Unknown];

fn caller() -> CreatedBy {
    CreatedBy {
        workspace_id: "dev".into(),
        thread_id: "thread-1".into(),
    }
}

fn workspace(id: &str, origin: WorkspaceOrigin) -> Workspace {
    let mut ws = Workspace::gateway_provisioned(id.into(), id.into(), 9100);
    ws.cleanup.origin = origin;
    ws
}

fn state() -> (tempfile::TempDir, GatewayState) {
    let temp = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(temp.path().to_owned()));
    (temp, state)
}

enum Caller {
    Engine,
    Agent,
    Device,
}

async fn call(
    state: &GatewayState,
    caller: Caller,
    uri: &str,
    body: Value,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    match caller {
        Caller::Engine => {
            request
                .extensions_mut()
                .insert(auth::AuthenticatedLocalProcess);
        }
        Caller::Agent => {
            request
                .extensions_mut()
                .insert(auth::AuthenticatedLocalProcess);
            request.headers_mut().insert(
                auth::HEADER_AGENT_ORIGIN_TOKEN,
                HeaderValue::from_static("subprocess"),
            );
        }
        Caller::Device => {
            request.extensions_mut().insert(auth::AuthenticatedDevice {
                id: "device-1".into(),
                label: "My iPhone".into(),
            });
        }
    }
    let response = crate::control::router()
        .with_state(state.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn registered(state: &GatewayState, id: &str) -> Option<Workspace> {
    state.registry_snapshot_for_test().get(id).cloned()
}

#[tokio::test]
async fn only_a_local_engine_reaches_the_broker() {
    let (_temp, state) = state();
    state.register_for_test(workspace("fixture", WorkspaceOrigin::Unknown));
    for who in [Caller::Agent, Caller::Device] {
        let (status, _) = call(
            &state,
            who,
            "/broker/workspaces/fixture/classify",
            json!({"origin": "demo"}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    assert_eq!(
        registered(&state, "fixture").unwrap().cleanup.origin,
        WorkspaceOrigin::Unknown
    );
}

#[tokio::test]
async fn the_broker_has_no_restart_route() {
    let (_temp, state) = state();
    state.register_for_test(workspace("fixture", WorkspaceOrigin::Agent));
    for uri in [
        "/broker/workspaces/fixture/restart",
        "/broker/restart",
        "/broker/gateway/reload",
    ] {
        let (status, _) = call(&state, Caller::Engine, uri, json!({})).await;
        assert!(
            matches!(
                status,
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            ),
            "{uri} answered {status}"
        );
    }
}

#[tokio::test]
async fn delete_refuses_a_user_or_unknown_workspace_and_leaves_it() {
    let (_temp, state) = state();
    for origin in PROTECTED {
        let id = format!("ws-{origin:?}").to_lowercase();
        state.register_for_test(workspace(&id, origin));
        let (status, body) = call(
            &state,
            Caller::Engine,
            &format!("/broker/workspaces/{id}/delete"),
            json!({"caller": caller()}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body.contains("origin agent, demo, eval or e2e"), "{body}");
        assert!(registered(&state, &id).is_some());
        assert!(!state.app_data().join("deleted").exists());
    }
}

#[test]
fn an_agent_may_delete_exactly_the_machine_made_origins() {
    for origin in MACHINE_MADE {
        let actor = crate::server::DeleteAuthority::Agent(caller())
            .authorize(&workspace("fixture", origin))
            .unwrap();
        assert_eq!(
            actor,
            crate::cleanup_audit::Actor::Agent {
                workspace_id: "dev".into(),
                thread_id: "thread-1".into(),
            }
        );
    }
    for origin in PROTECTED {
        assert!(crate::server::DeleteAuthority::Agent(caller())
            .authorize(&workspace("fixture", origin))
            .is_err());
    }
}

#[tokio::test]
async fn trash_restore_refuses_a_protected_entry_before_touching_it() {
    for origin in PROTECTED.into_iter().chain(MACHINE_MADE) {
        let (temp, state) = state();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let (entry, root) = crate::trash::Entry::prepare(
            &state.app_data().join("deleted"),
            &workspace("fixture", origin),
            source,
            |_| Ok(()),
        )
        .unwrap();
        entry.save(&root).unwrap();
        let id = root.file_name().unwrap().to_str().unwrap().to_owned();
        let (status, body) = call(
            &state,
            Caller::Engine,
            &format!("/broker/trash/{id}/restore"),
            json!({"caller": caller()}),
        )
        .await;
        if origin.is_machine_made() {
            assert_ne!(status, StatusCode::FORBIDDEN, "{origin:?}: {body}");
        } else {
            assert_eq!(status, StatusCode::FORBIDDEN, "{origin:?}: {body}");
            assert!(root.join("entry.json").exists());
        }
    }
}

#[tokio::test]
async fn trash_restore_refuses_a_folder_or_database_entry() {
    let (_temp, state) = state();
    let (status, _) = call(
        &state,
        Caller::Engine,
        "/broker/trash/database-abc/restore",
        json!({"caller": caller()}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn classify_moves_only_an_unknown_entry() {
    let (_temp, state) = state();
    state.register_for_test(workspace("legacy", WorkspaceOrigin::Unknown));
    state.register_for_test(workspace("mine", WorkspaceOrigin::User));
    let classify = |id: &'static str, origin: &'static str| {
        let state = state.clone();
        async move {
            call(
                &state,
                Caller::Engine,
                &format!("/broker/workspaces/{id}/classify"),
                json!({ "origin": origin }),
            )
            .await
            .0
        }
    };
    assert_eq!(classify("legacy", "unknown").await, StatusCode::BAD_REQUEST);
    assert_eq!(classify("legacy", "demo").await, StatusCode::NO_CONTENT);
    assert_eq!(
        registered(&state, "legacy").unwrap().cleanup.origin,
        WorkspaceOrigin::Demo
    );
    assert_eq!(classify("legacy", "user").await, StatusCode::CONFLICT);
    assert_eq!(classify("mine", "demo").await, StatusCode::CONFLICT);
    assert_eq!(
        registered(&state, "mine").unwrap().cleanup.origin,
        WorkspaceOrigin::User
    );
    let saved = crate::registry::Registry::load(&state.registry_path_for_test()).unwrap();
    assert_eq!(
        saved.get("legacy").unwrap().cleanup.origin,
        WorkspaceOrigin::Demo
    );
}

#[test]
fn a_broker_create_is_stamped_agent_made_with_its_creator_and_autostart_off() {
    let (_temp, state) = state();
    let ws = state
        .reserve_new_workspace("Fixture One", Some("fixture-one"), |ws| {
            ws.agent_made(false, caller())
        })
        .unwrap();
    let saved = registered(&state, "fixture-one").unwrap();
    assert_eq!(saved, ws);
    assert_eq!(saved.cleanup.origin, WorkspaceOrigin::Agent);
    assert!(saved.cleanup.ephemeral);
    assert_eq!(saved.cleanup.created_by, Some(caller()));
    assert!(!saved.autostart);
}

#[test]
fn a_broker_create_refuses_a_taken_slug_or_name_rather_than_suffixing() {
    let (_temp, state) = state();
    state.register_for_test(workspace("taken", WorkspaceOrigin::User));
    let stamp = |ws: Workspace| ws.agent_made(false, caller());
    let slug = state
        .reserve_new_workspace("Fresh name", Some("taken"), stamp)
        .unwrap_err();
    assert_eq!(slug.into_response().status(), StatusCode::CONFLICT);
    let name = state
        .reserve_new_workspace("taken", Some("fresh"), stamp)
        .unwrap_err();
    assert_eq!(name.into_response().status(), StatusCode::CONFLICT);
    let invalid = state
        .reserve_new_workspace("Fresh", Some("Not A Slug"), stamp)
        .unwrap_err();
    assert_eq!(invalid.into_response().status(), StatusCode::BAD_REQUEST);
    assert!(registered(&state, "taken-2").is_none());
    assert!(registered(&state, "fresh").is_none());
}

use axum::response::IntoResponse as _;
