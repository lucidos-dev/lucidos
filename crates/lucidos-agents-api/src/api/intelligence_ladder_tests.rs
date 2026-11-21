//! Only the owner edits an intelligence ladder freely. An agent edits one by
//! spending the owner's `edit-intelligence-ladder` permission (ADR 0750).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::api::gateway_stamp::test_support::stamped_device_headers;
use crate::api::permission_requests::tests::{
    agent_headers, agent_thread, answer_and_tap, ask_prepared, permission_test_state,
};
use crate::engine::permission::menu::{
    ALLOW_FOR_THIS_THREAD_LABEL, ALLOW_ONCE_LABEL, ALWAYS_ALLOW_LABEL,
};
use crate::engine::permission::Resolution;
use crate::test_support::{boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db};

const SORT: &str = "/intelligence-ladder/sort";

async fn call(state: &AppState, uri: &str, headers: HeaderMap, body: Value) -> (StatusCode, Value) {
    let method = if body.is_null() { "GET" } else { "POST" };
    send(router(), state, method, uri, headers, body).await
}

/// One request through the `/api/v1/models` routes.
async fn models(
    state: &AppState,
    method: &str,
    uri: &str,
    headers: HeaderMap,
    body: Value,
) -> (StatusCode, Value) {
    let router = crate::api::settings::router();
    send(router, state, method, uri, headers, body).await
}

async fn send(
    router: axum::Router<AppState>,
    state: &AppState,
    method: &str,
    uri: &str,
    headers: HeaderMap,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(if body.is_null() {
            String::new()
        } else {
            body.to_string()
        }))
        .unwrap();
    request.headers_mut().extend(headers);
    let response: Response = router
        .with_state(state.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn owner() -> HeaderMap {
    stamped_device_headers("owner-phone")
}

fn sort_body(reason: &str) -> Value {
    json!({ "surface": "claude-code", "reason": reason })
}

#[tokio::test]
async fn an_agent_edits_a_ladder_only_while_it_holds_the_owners_permission() {
    if !in_engine_child(
        "api::intelligence_ladder::tests::an_agent_edits_a_ladder_only_while_it_holds_the_owners_permission",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    let bus = state.engine.event_bus.clone();
    let thread = agent_thread(&bus).await;
    let agent = || agent_headers(Some(thread));

    let (status, body) = call(&state, SORT, owner(), sort_body("")).await;
    assert_eq!(status, StatusCode::OK, "the owner edits freely: {body}");

    let (status, body) = call(&state, SORT, agent(), sort_body("")).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an agent gives a reason: {body}"
    );

    let asked = |body: &Value| {
        assert_eq!(body["reason"], "permission_required", "{body}");
        body["request_id"].as_str().unwrap().to_owned()
    };
    let allow = |request_id: String, label: &'static str| {
        let bus = bus.clone();
        async move {
            let card = ask_prepared(&bus, thread, &request_id).await;
            let labels: Vec<&str> = card.options.iter().map(|o| o.label.as_str()).collect();
            assert!(labels.contains(&ALLOW_FOR_THIS_THREAD_LABEL), "{labels:?}");
            assert!(!labels.contains(&ALWAYS_ALLOW_LABEL), "{labels:?}");
            let picked = card.pick(label);
            assert!(matches!(
                answer_and_tap(&bus, thread, &card, picked).await,
                Resolution::Granted(_)
            ));
        }
    };

    let (status, body) = call(&state, SORT, agent(), sort_body("The swap.")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    allow(asked(&body), ALLOW_ONCE_LABEL).await;
    let (status, body) = call(&state, SORT, agent(), sort_body("The swap.")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The once permission is spent, so the next edit asks again.
    let (status, body) = call(&state, SORT, agent(), sort_body("The swap.")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    allow(asked(&body), ALLOW_FOR_THIS_THREAD_LABEL).await;
    for _ in 0..2 {
        let (status, body) = call(&state, SORT, agent(), sort_body("")).await;
        assert_eq!(status, StatusCode::OK, "this thread holds it: {body}");
    }

    // Another thread holds nothing.
    let other = agent_thread(&bus).await;
    let (status, body) = call(&state, SORT, agent_headers(Some(other)), sort_body("Why.")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_refused_edit_gives_the_once_permission_back() {
    if !in_engine_child(
        "api::intelligence_ladder::tests::a_refused_edit_gives_the_once_permission_back",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    let bus = state.engine.event_bus.clone();
    let thread = agent_thread(&bus).await;
    let agent = || agent_headers(Some(thread));

    let (status, body) = call(&state, SORT, agent(), sort_body("The swap.")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let request_id = body["request_id"].as_str().unwrap().to_owned();
    let card = ask_prepared(&bus, thread, &request_id).await;
    let picked = card.pick(ALLOW_ONCE_LABEL);
    assert!(matches!(
        answer_and_tap(&bus, thread, &card, picked).await,
        Resolution::Granted(_)
    ));

    // The default's model stays on, so this edit is refused and changes nothing.
    let ladder = intelligence_ladder::load(
        &state.pool,
        &state.engine.model_routes(),
        LadderSurface::ClaudeCode,
    )
    .await;
    let default = ladder.default_intelligence_rung.clone().unwrap().model;
    let remove = json!({ "surface": "claude-code", "model": default, "reason": "The swap." });
    let (status, body) = call(&state, "/intelligence-ladder/remove-model", agent(), remove).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = call(&state, SORT, agent(), sort_body("The swap.")).await;
    assert_eq!(status, StatusCode::OK, "the grant came back: {body}");
    let (status, body) = call(&state, SORT, agent(), sort_body("The swap.")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "and is spent once: {body}");
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_owner_sets_the_rungs_and_removes_a_model() {
    if !in_engine_child(
        "api::intelligence_ladder::tests::the_owner_sets_the_rungs_and_removes_a_model",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;

    let (status, ladder) = call(
        &state,
        "/intelligence-ladder?surface=claude-code",
        HeaderMap::new(),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ladder}");
    let default_model = ladder["default_rung"]["model"].as_str().unwrap().to_owned();
    let on: Vec<Value> = ladder["rungs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["on"] == true)
        .map(|r| json!({ "model": r["model"], "effort": r["effort"] }))
        .collect();
    let dropped = on
        .iter()
        .position(|r| r["model"] != default_model.as_str())
        .expect("a rung runs another model than the default");
    let mut kept = on.clone();
    let gone = kept.remove(dropped);

    let set = json!({ "surface": "claude-code", "rungs": kept });
    let (status, body) = call(&state, "/intelligence-ladder/rungs", owner(), set).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rungs"], json!(kept));

    let remove = |model: &str| json!({ "surface": "claude-code", "model": model });
    let (status, body) = call(
        &state,
        "/intelligence-ladder/remove-model",
        owner(),
        remove(&default_model),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "never the default: {body}");

    let model = kept
        .iter()
        .find(|r| r["model"] != default_model.as_str())
        .map_or(gone["model"].clone(), |r| r["model"].clone());
    let model = model.as_str().unwrap();
    let (status, body) = call(
        &state,
        "/intelligence-ladder/remove-model",
        owner(),
        remove(model),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let left = body["rungs"].as_array().unwrap();
    assert!(left.iter().all(|r| r["model"] != model), "{body}");
    teardown_test_db(&db).await;
}

async fn chat_ladder(state: &AppState) -> intelligence_ladder::IntelligenceLadder {
    intelligence_ladder::load(
        &state.pool,
        &state.engine.model_routes(),
        LadderSurface::Chat,
    )
    .await
}

/// A model on the chat ladder that may leave it: not the default's. Chat
/// stores no default rung here, so its default selection names that model.
async fn removable_laddered_model(state: &AppState) -> String {
    let ladder = chat_ladder(state).await;
    let offer =
        intelligence_ladder::ChatOffer::load(&state.pool, state.engine.model_routes()).await;
    let default = intelligence_ladder::default_chat_selection(&state.pool, &offer)
        .await
        .model;
    ladder
        .pairs()
        .into_iter()
        .map(|r| r.model)
        .find(|m| *m != default)
        .expect("a model on the chat ladder besides the default's")
}

/// The request id a `permission_required` answer gives to ask with.
fn asked(body: &Value) -> String {
    assert_eq!(body["reason"], "permission_required", "{body}");
    body["request_id"].as_str().unwrap().to_owned()
}

async fn allow_once(
    bus: &crate::engine::event_bus::EventBus,
    thread: uuid::Uuid,
    request_id: &str,
) {
    let card = ask_prepared(bus, thread, request_id).await;
    let picked = card.pick(ALLOW_ONCE_LABEL);
    assert!(matches!(
        answer_and_tap(bus, thread, &card, picked).await,
        Resolution::Granted(_)
    ));
}

fn toggle(on: bool, reason: &str) -> Value {
    json!({ "on_chat_ladder": on, "reason": reason })
}

#[tokio::test]
async fn an_agent_toggles_a_model_on_the_chat_ladder_only_with_the_owners_permission() {
    if !in_engine_child(
        "api::intelligence_ladder::tests::an_agent_toggles_a_model_on_the_chat_ladder_only_with_the_owners_permission",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    let bus = state.engine.event_bus.clone();
    let thread = agent_thread(&bus).await;
    let agent = || agent_headers(Some(thread));
    let model = removable_laddered_model(&state).await;
    let uri = format!("/models?id={model}");
    let listed = || async { chat_ladder(&state).await.lists_model(&model) };

    let (status, body) = models(&state, "PUT", &uri, agent(), toggle(false, "")).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an agent gives a reason: {body}"
    );

    let (status, body) = models(&state, "PUT", &uri, agent(), toggle(false, "The swap.")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let request_id = asked(&body);
    assert!(listed().await, "no permission changes nothing");

    allow_once(&bus, thread, &request_id).await;
    let (status, body) = models(&state, "PUT", &uri, agent(), toggle(false, "The swap.")).await;
    assert_eq!(
        (status, &body["success"]),
        (StatusCode::OK, &json!(true)),
        "{body}"
    );
    assert!(!listed().await, "the once grant paid for one edit");

    let (status, body) = models(&state, "PUT", &uri, agent(), toggle(true, "Back on.")).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the next edit asks again: {body}"
    );
    asked(&body);
    assert!(!listed().await);

    let (status, body) = models(&state, "PUT", &uri, owner(), toggle(true, "")).await;
    assert_eq!(
        (status, &body["success"]),
        (StatusCode::OK, &json!(true)),
        "{body}"
    );
    assert!(listed().await, "the owner edits freely");
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_refused_model_edit_gives_the_once_permission_back() {
    if !in_engine_child(
        "api::intelligence_ladder::tests::a_refused_model_edit_gives_the_once_permission_back",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    let bus = state.engine.event_bus.clone();
    let thread = agent_thread(&bus).await;
    let agent = || agent_headers(Some(thread));
    let unknown = "/models?id=no-such-model";
    let off_other = format!("/models?id={}", removable_laddered_model(&state).await);

    let (status, body) = models(&state, "PUT", unknown, agent(), toggle(false, "Why.")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    allow_once(&bus, thread, &asked(&body)).await;

    // No such model, so this edit is refused and changes nothing.
    let (status, body) = models(&state, "PUT", unknown, agent(), toggle(false, "Why.")).await;
    assert_eq!(
        (status, &body["success"]),
        (StatusCode::OK, &json!(false)),
        "{body}"
    );

    let (status, body) = models(&state, "PUT", &off_other, agent(), toggle(false, "Why.")).await;
    assert_eq!(
        (status, &body["success"]),
        (StatusCode::OK, &json!(true)),
        "the grant came back: {body}"
    );
    let (status, body) = models(&state, "PUT", &off_other, agent(), toggle(true, "Why.")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "and is spent once: {body}");
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn deleting_a_laddered_model_asks_the_owner_first() {
    if !in_engine_child(
        "api::intelligence_ladder::tests::deleting_a_laddered_model_asks_the_owner_first",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    let bus = state.engine.event_bus.clone();
    let thread = agent_thread(&bus).await;
    let agent = || agent_headers(Some(thread));

    // A user model served the way a laddered builtin is, so it can join.
    let other = removable_laddered_model(&state).await;
    let served = crate::core::ModelStore::get(&state.pool, &other)
        .await
        .unwrap()
        .unwrap();
    let id = "user-copy";
    let routes: Vec<Value> = served
        .routes
        .iter()
        .map(|r| json!({ "provider": r.provider, "id": r.wire_id(&served.id) }))
        .collect();
    let add = json!({ "id": id, "routes": routes });
    let (status, body) = models(&state, "POST", "/models", owner(), add).await;
    assert_eq!(
        (status, &body["success"]),
        (StatusCode::OK, &json!(true)),
        "{body}"
    );
    let registry = state.engine.model_routes().registry;
    for _ in 0..100 {
        if crate::llm::model_registry::routing_for(&registry, id).is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let on = format!("/models?id={id}");
    let (status, body) = models(&state, "PUT", &on, owner(), toggle(true, "")).await;
    assert_eq!(
        (status, &body["success"]),
        (StatusCode::OK, &json!(true)),
        "{body}"
    );
    assert!(chat_ladder(&state).await.lists_model(id));

    let delete = format!("/models?id={id}&reason=Retired.");
    let (status, body) = models(&state, "DELETE", &delete, agent(), Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let request_id = asked(&body);
    let kept = crate::core::ModelStore::get(&state.pool, id).await.unwrap();
    assert!(kept.is_some(), "no permission deletes nothing");

    allow_once(&bus, thread, &request_id).await;
    let (status, body) = models(&state, "DELETE", &delete, agent(), Value::Null).await;
    assert_eq!(
        (status, &body["success"]),
        (StatusCode::OK, &json!(true)),
        "{body}"
    );
    let gone = crate::core::ModelStore::get(&state.pool, id).await.unwrap();
    assert!(gone.is_none(), "the once grant paid for the delete");
    teardown_test_db(&db).await;
}

/// Run one `manage_models` action as the Lucidos Agent in `thread` would,
/// through the engine's real `/api/v1` router.
async fn manage_models(
    state: &AppState,
    thread: uuid::Uuid,
    args: Value,
) -> Result<String, String> {
    use crate::engine::tools::ToolDispatchExt;
    let tool = crate::capability_manifest::domains()
        .iter()
        .find(|d| d.name == "models")
        .unwrap()
        .tool_name;
    if state.engine.api_router().is_none() {
        state.engine.set_api_router(crate::api::api_routes(state));
    }
    state
        .engine
        .execute_tool(tool, &args, "", uuid::Uuid::new_v4(), None, thread)
        .await
}

/// The request id a tool refusal tells the agent to ask with.
fn tool_asked(refusal: &str) -> String {
    assert!(refusal.starts_with("Error: Nothing changed"), "{refusal}");
    refusal.split('`').nth(1).unwrap().to_owned()
}

fn ladder_action(action: &str, model: &str, reason: &str) -> Value {
    json!({ "action": action, "id": model, "reason": reason })
}

#[tokio::test]
async fn the_agents_model_tool_edits_the_ladder_only_with_the_owners_permission() {
    if !in_engine_child(
        "api::intelligence_ladder::tests::the_agents_model_tool_edits_the_ladder_only_with_the_owners_permission",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    let bus = state.engine.event_bus.clone();
    let thread = agent_thread(&bus).await;
    let model = removable_laddered_model(&state).await;
    let listed = || async { chat_ladder(&state).await.lists_model(&model) };

    let err = manage_models(&state, thread, ladder_action("disable", &model, ""))
        .await
        .unwrap_err();
    assert!(err.contains("400"), "an agent gives a reason: {err}");

    let disable = ladder_action("disable", &model, "The swap.");
    let err = manage_models(&state, thread, disable.clone())
        .await
        .unwrap_err();
    let request_id = tool_asked(&err);
    assert!(listed().await, "no permission changes nothing");

    allow_once(&bus, thread, &request_id).await;
    manage_models(&state, thread, disable).await.unwrap();
    assert!(!listed().await, "the once grant paid for one edit");

    let enable = ladder_action("enable", &model, "Back on.");
    let err = manage_models(&state, thread, enable).await.unwrap_err();
    tool_asked(&err);
    assert!(!listed().await, "the next edit asks again");

    let list = manage_models(&state, thread, json!({ "action": "list" }))
        .await
        .unwrap();
    let list: Value = serde_json::from_str(&list).unwrap();
    let row = list["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == model.as_str())
        .unwrap();
    assert_eq!(row["on_chat_ladder"], false, "list reads without asking");
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_refused_model_tool_edit_gives_the_once_permission_back() {
    if !in_engine_child(
        "api::intelligence_ladder::tests::a_refused_model_tool_edit_gives_the_once_permission_back",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    let bus = state.engine.event_bus.clone();
    let thread = agent_thread(&bus).await;
    let unknown = ladder_action("disable", "no-such-model", "Why.");
    let other = removable_laddered_model(&state).await;

    let err = manage_models(&state, thread, unknown.clone())
        .await
        .unwrap_err();
    allow_once(&bus, thread, &tool_asked(&err)).await;

    // No such model, so this edit is refused and changes nothing.
    let err = manage_models(&state, thread, unknown).await.unwrap_err();
    assert!(err.contains("not found"), "{err}");

    let off = ladder_action("disable", &other, "Why.");
    manage_models(&state, thread, off).await.unwrap();
    let on = ladder_action("enable", &other, "Why.");
    let err = manage_models(&state, thread, on).await.unwrap_err();
    tool_asked(&err);
    teardown_test_db(&db).await;
}
