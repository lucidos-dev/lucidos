use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::Request;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::api::gateway_stamp::test_support::stamped_device_headers;
use crate::api::permission_requests::tests::{
    agent_headers, agent_thread, answer_and_tap, ask_prepared, permission_test_state,
};
use crate::core::event_hooks::{
    EventHooks, HookArgument, HookDefinition, HookedEventKind, ASKED_EVENT_TYPES, HOOK_AGGREGATE,
};
use crate::core::subprocess_origin::HEADER_DEVICE_ID;
use crate::engine::permission::menu::ALLOW_ONCE_LABEL;
use crate::engine::permission::Resolution;
use crate::test_support::{boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db};

const GATE: &str = "gate";
const HOOKED: &str = "MessageReceived";

fn gate_definition() -> HookDefinition {
    HookDefinition {
        arguments: Default::default(),
        name: "Gate".into(),
        event_type: HOOKED.into(),
        condition: None,
        script: "check.py".into(),
        timeout_secs: None,
    }
}

fn gate_listing(content_hash: &str) -> HookListing {
    HookListing::Valid {
        slug: GATE.into(),
        event_kind: HookedEventKind::Thread,
        definition: gate_definition(),
        content_hash: content_hash.into(),
    }
}

fn armed(slug: &str, content_hash: &str) -> ArmedHook {
    ArmedHook {
        slug: slug.into(),
        event_type: HOOKED.into(),
        content_hash: content_hash.into(),
        armed_at: chrono::Utc::now(),
        actor: None,
    }
}

#[test]
fn a_row_is_its_listing_plus_its_arming() {
    let listed = rows(
        vec![(gate_listing("abc"), None)],
        vec![armed(GATE, "abc")],
        &[HOOKED],
    );
    let row = serde_json::to_value(&listed).unwrap();
    assert_eq!(row[0]["status"], "valid");
    assert_eq!(row[0]["slug"], GATE);
    assert_eq!(row[0]["definition"]["event_type"], HOOKED);
    assert_eq!(row[0]["arm_state"], "armed");
    assert_eq!(row[0]["armed"]["content_hash"], "abc");
    assert_eq!(row[0]["content_hash"], "abc");
    assert_eq!(row[0]["arm_refusal"], "It is armed already.");
    assert_eq!(row[0]["remove_refusal"], Value::Null);
    assert!(row[0].get("choices_timeout_secs").is_none());
}

#[test]
fn only_an_armed_first_message_hook_offers_choices_with_its_effective_timeout() {
    let mut listing = gate_listing("abc");
    let HookListing::Valid { definition, .. } = &mut listing else {
        panic!("expected a valid hook");
    };
    definition.arguments.insert(
        "ticket".into(),
        HookArgument {
            label: "Ticket".into(),
            choices_script: "choices.py".into(),
            create_label: None,
        },
    );
    let row = rows(
        vec![(listing.clone(), None)],
        vec![armed(GATE, "abc")],
        &[HOOKED],
    );
    assert_eq!(
        row[0].choices_timeout_secs,
        Some(effective_timeout(None).as_secs())
    );
    let row = rows(vec![(listing, None)], vec![armed(GATE, "old")], &[HOOKED]);
    assert_eq!(row[0].choices_timeout_secs, None);
}

#[test]
fn an_armed_hook_whose_folder_left_lists_as_missing_and_changed() {
    let listed = rows(
        vec![(gate_listing("abc"), None)],
        vec![armed("zulu", "abc"), armed("alpha", "def")],
        &[HOOKED],
    );
    let rows = serde_json::to_value(&listed).unwrap();
    let slugs: Vec<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, ["alpha", GATE, "zulu"]);
    assert_eq!(rows[0]["status"], "missing");
    assert_eq!(rows[0]["arm_state"], "changed");
    assert_eq!(rows[0]["content_hash"], Value::Null);
    assert!(rows[0]["arm_refusal"]
        .as_str()
        .unwrap()
        .contains("folder is gone"));
    assert_eq!(rows[1]["arm_state"], "disarmed");
    assert_eq!(rows[1]["armed"], Value::Null);
    assert_eq!(rows[1]["arm_refusal"], Value::Null);
}

#[test]
fn a_hook_arms_only_when_valid_asked_and_not_armed_already() {
    let refusal = |listing: HookListing, asked: &[&str]| {
        rows(vec![(listing, None)], vec![], asked)
            .remove(0)
            .arm_refusal
    };
    assert_eq!(refusal(gate_listing("abc"), &[HOOKED]), None);
    let invalid = HookListing::Invalid {
        slug: GATE.into(),
        definition: None,
        problems: vec!["broken".into()],
    };
    assert!(refusal(invalid, &[HOOKED]).unwrap().contains("problems"));
    assert_eq!(refusal(gate_listing("abc"), ASKED_EVENT_TYPES), None);
    assert!(refusal(gate_listing("abc"), &["ChangeProposed"])
        .unwrap()
        .contains(HOOKED));
    let changed = rows(
        vec![(gate_listing("new"), None)],
        vec![armed(GATE, "old")],
        &[HOOKED],
    )
    .remove(0);
    assert_eq!(changed.arm_state, ArmState::Changed);
    assert_eq!(changed.armable(&[HOOKED]).unwrap(), (HOOKED, "new"));
}

#[test]
fn a_slug_is_one_plain_path_segment() {
    for slug in ["gate", "My Hook", "v2.gate"] {
        assert!(checked_slug(slug).is_ok(), "{slug}");
    }
    for slug in ["", "..", ".git", "a/b", "a\\b", "a`b", "a\nb"] {
        assert!(checked_slug(slug).is_err(), "{slug:?}");
    }
}

#[cfg(unix)]
#[test]
fn a_removal_is_refused_only_when_the_entry_cannot_be_named_or_opened() {
    let workspace = tempfile::tempdir().unwrap();
    let ws = workspace.path();
    let root = ws.join(crate::core::DATA_DIR).join(HOOKS_DIR);
    std::fs::create_dir_all(root.join(GATE)).unwrap();
    std::fs::create_dir_all(root.join("a`b")).unwrap();
    std::os::unix::fs::symlink(ws, root.join("out")).unwrap();
    assert_eq!(removal_refusal(ws, GATE), None);
    assert_eq!(removal_refusal(ws, "out"), None);
    assert!(removal_refusal(ws, "a`b")
        .unwrap()
        .contains("outside Lucidos"));
    assert!(removal_refusal(ws, "gone")
        .unwrap()
        .contains("cannot be opened"));

    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(elsewhere.path().join(GATE)).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), &root).unwrap();
    assert!(removal_refusal(ws, GATE).unwrap().contains("links outside"));
}

#[test]
fn a_refused_removal_is_refused_before_anyone_is_asked() {
    let row = rows(vec![(gate_listing("abc"), Some("why".into()))], vec![], &[]).remove(0);
    let Err(refusal) = Plan::of(Verb::Remove, GATE, Some(&row), &[]) else {
        panic!("a refused removal plans nothing");
    };
    assert_eq!(refusal.status, StatusCode::CONFLICT);
    assert_eq!(refusal.message, "why");
}

struct FakeHooks {
    digest: Mutex<String>,
    asked: &'static [&'static str],
}

impl FakeHooks {
    fn edit(&self, digest: &str) {
        *self.digest.lock().unwrap() = digest.into();
    }
}

fn gate_dir(workspace: &FsPath) -> PathBuf {
    workspace
        .join(crate::core::DATA_DIR)
        .join(HOOKS_DIR)
        .join(GATE)
}

impl EventHooks for FakeHooks {
    fn list_definitions(&self, workspace: &FsPath) -> Result<Vec<HookListing>, String> {
        Ok(gate_dir(workspace)
            .is_dir()
            .then(|| gate_listing(&self.digest.lock().unwrap()))
            .into_iter()
            .collect())
    }

    fn asked_event_types(&self) -> &'static [&'static str] {
        self.asked
    }
}

fn write_gate(workspace: &FsPath) {
    std::fs::create_dir_all(gate_dir(workspace)).unwrap();
    std::fs::write(gate_dir(workspace).join("hook.toml"), "name = \"Gate\"\n").unwrap();
}

async fn hooks_state(
    engine: crate::engine::types::SharedEngine,
    workspace: &FsPath,
    asked: &'static [&'static str],
) -> (AppState, Arc<FakeHooks>) {
    write_gate(workspace);
    let hooks = Arc::new(FakeHooks {
        digest: Mutex::new("first".into()),
        asked,
    });
    let mut state = permission_test_state(engine, workspace.to_path_buf()).await;
    state.event_hooks = hooks.clone();
    (state, hooks)
}

fn request(method: &str, uri: &str, headers: HeaderMap, body: Value) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.headers_mut().extend(headers);
    request
}

async fn read(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    headers: HeaderMap,
    body: Value,
) -> (StatusCode, Value) {
    let response = router()
        .with_state(state.clone())
        .oneshot(request(method, uri, headers, body))
        .await
        .unwrap();
    read(response).await
}

async fn listed(state: &AppState) -> Vec<Value> {
    let (status, body) = call(state, "GET", "/hooks", HeaderMap::new(), Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["hooks"].as_array().unwrap().clone()
}

async fn arming_events(state: &AppState) -> Vec<String> {
    sqlx::query_scalar("SELECT event_type FROM events WHERE aggregate = $1 ORDER BY sequence")
        .bind(HOOK_AGGREGATE)
        .fetch_all(&state.pool)
        .await
        .unwrap()
}

fn owner() -> HeaderMap {
    stamped_device_headers("owner-phone")
}

#[tokio::test]
async fn the_owner_arms_disarms_and_removes_a_hook() {
    if !in_engine_child("api::hooks::tests::the_owner_arms_disarms_and_removes_a_hook").await {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let (state, hooks) = hooks_state(engine, workspace.path(), &[HOOKED]).await;
    let (arm, disarm) = ("/hooks/gate/arm", "/hooks/gate/disarm");

    assert_eq!(listed(&state).await[0]["arm_state"], "disarmed");
    let mut app = owner();
    app.insert("sec-fetch-site", "same-origin".parse().unwrap());
    app.insert(
        "referer",
        "https://localhost/app/demo/index.html".parse().unwrap(),
    );
    let (status, body) = call(&state, "POST", arm, app, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, row) = call(&state, "POST", arm, owner(), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{row}");
    assert_eq!(row["arm_state"], "armed");
    assert_eq!(row["armed"]["actor"]["device_id"], "owner-phone", "{row}");
    let (status, body) = call(&state, "POST", arm, owner(), json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    hooks.edit("second");
    assert_eq!(listed(&state).await[0]["arm_state"], "changed");
    let shown = |hash: &str| json!({ "content_hash": hash });
    let (status, body) = call(&state, "POST", arm, owner(), shown("first")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("changed"),
        "{body}"
    );
    let (status, row) = call(&state, "POST", arm, owner(), shown("second")).await;
    assert_eq!(status, StatusCode::OK, "{row}");
    assert_eq!(row["armed"]["content_hash"], "second");

    let (status, row) = call(&state, "POST", disarm, owner(), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{row}");
    assert_eq!(row["arm_state"], "disarmed");
    let (status, body) = call(&state, "POST", disarm, owner(), json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let (status, _) = call(&state, "POST", arm, owner(), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    std::fs::remove_dir_all(gate_dir(workspace.path())).unwrap();
    let rows = listed(&state).await;
    assert_eq!(
        (rows[0]["status"].clone(), rows[0]["arm_state"].clone()),
        (json!("missing"), json!("changed"))
    );
    let (status, body) = call(&state, "DELETE", "/hooks/gate", owner(), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(listed(&state).await.is_empty());

    write_gate(workspace.path());
    let (status, _) = call(&state, "POST", arm, owner(), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(&state, "DELETE", "/hooks/gate", owner(), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!gate_dir(workspace.path()).exists());
    assert!(listed(&state).await.is_empty());
    let (status, body) = call(&state, "DELETE", "/hooks/gate", owner(), json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    assert_eq!(
        arming_events(&state).await,
        [
            "EventHookArmed",
            "EventHookArmed",
            "EventHookDisarmed",
            "EventHookArmed",
            "EventHookRemoved",
            "EventHookArmed",
            "EventHookRemoved",
        ]
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_agent_acts_only_on_a_once_permission_for_the_exact_content() {
    if !in_engine_child(
        "api::hooks::tests::an_agent_acts_only_on_a_once_permission_for_the_exact_content",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let (state, hooks) = hooks_state(engine, workspace.path(), &[HOOKED]).await;
    let bus = state.engine.event_bus.clone();
    let thread = agent_thread(&bus).await;
    let agent = || agent_headers(Some(thread));
    let reason = json!({ "reason": "The plan rule needs it." });

    let (status, body) = call(&state, "POST", "/hooks/gate/arm", agent(), json!({})).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an agent gives a reason: {body}"
    );

    let allow_once = |request_id: String| {
        let bus = bus.clone();
        async move {
            let card = ask_prepared(&bus, thread, &request_id).await;
            let picked = card.pick(ALLOW_ONCE_LABEL);
            assert!(matches!(
                answer_and_tap(&bus, thread, &card, picked).await,
                Resolution::Granted(_)
            ));
        }
    };
    let asked = |body: &Value| {
        assert_eq!(body["reason"], "permission_required", "{body}");
        body["request_id"].as_str().unwrap().to_owned()
    };

    let (status, body) = call(&state, "POST", "/hooks/gate/arm", agent(), reason.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(arming_events(&state).await.is_empty());
    allow_once(asked(&body)).await;
    let (status, row) = call(&state, "POST", "/hooks/gate/arm", agent(), reason.clone()).await;
    assert_eq!(status, StatusCode::OK, "{row}");
    assert_eq!(row["armed"]["actor"]["mode"], "agent", "{row}");

    let (status, body) = call(
        &state,
        "POST",
        "/hooks/gate/disarm",
        agent(),
        reason.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    allow_once(asked(&body)).await;
    let (status, row) = call(
        &state,
        "POST",
        "/hooks/gate/disarm",
        agent(),
        reason.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{row}");

    let (_, body) = call(&state, "POST", "/hooks/gate/arm", agent(), reason.clone()).await;
    let first = asked(&body);
    allow_once(first.clone()).await;
    hooks.edit("edited");
    let (status, body) = call(&state, "POST", "/hooks/gate/arm", agent(), reason.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_ne!(asked(&body), first);
    assert_eq!(
        arming_events(&state).await,
        ["EventHookArmed", "EventHookDisarmed"]
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_unasked_hook_and_an_unattested_caller_are_refused() {
    if !in_engine_child("api::hooks::tests::an_unasked_hook_and_an_unattested_caller_are_refused")
        .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let (state, _) = hooks_state(engine.clone(), workspace.path(), &["ChangeProposed"]).await;
    let thread = agent_thread(&state.engine.event_bus).await;
    let reason = json!({ "reason": "The plan rule needs it." });

    for headers in [owner(), agent_headers(Some(thread))] {
        let (status, body) = call(&state, "POST", "/hooks/gate/arm", headers, reason.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(body["error"].as_str().unwrap().contains(HOOKED), "{body}");
    }
    assert!(listed(&state).await[0]["arm_refusal"]
        .as_str()
        .unwrap()
        .contains(HOOKED));

    let (state, _) = hooks_state(engine, workspace.path(), &[HOOKED]).await;
    let mut typed = HeaderMap::new();
    typed.insert(HEADER_DEVICE_ID, "owner-phone".parse().unwrap());
    let response = router()
        .with_state(state.clone())
        .oneshot(request("POST", "/hooks/gate/arm", typed, json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(arming_events(&state).await.is_empty());
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn refuses_choices_for_an_unarmed_hook() {
    if !in_engine_child("api::hooks::tests::refuses_choices_for_an_unarmed_hook").await {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let (state, _hooks) = hooks_state(engine, workspace.path(), &[HOOKED]).await;
    let (status, body) = call(
        &state,
        "GET",
        "/hooks/gate/arguments/ticket/choices?query=login",
        owner(),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("not armed"));
    drop(state);
    teardown_test_db(&db).await;
}
