//! Who may ask for a confinement run, and what the caller can steer.

use super::*;

/// The handler admits the thread's own agent before it reaches the engine.
/// `background_tasks_tests` proves what that check refuses.
#[test]
fn the_route_admits_only_the_threads_own_agent_first() {
    let src = include_str!("confinement_run.rs");
    let body = &src[src
        .find("pub(in crate::api) async fn run_confinement_tests(")
        .unwrap()..];
    let check = body
        .find("require_own_thread_agent(&headers, thread_id)?")
        .unwrap();
    let run = body.find(".run_confinement_tests_for_agent(").unwrap();
    assert!(check < run, "the caller check runs first");
}

/// The request carries no command, so no caller text reaches the spawn.
#[test]
fn the_request_carries_no_command() {
    let req: RunRequest =
        serde_json::from_value(serde_json::json!({ "command": "rm -rf ~", "card_id": "c1" }))
            .unwrap();
    assert!(matches!(req.asking(), Asking::Waiting { card_id: "c1" }));
    let plain: RunRequest = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(matches!(plain.asking(), Asking::Owner));
    let quiet: RunRequest =
        serde_json::from_value(serde_json::json!({ "ask_owner": false })).unwrap();
    assert!(matches!(quiet.asking(), Asking::Nobody));
    assert_eq!(
        crate::engine::agent_session::confinement_run::CONFINEMENT_COMMAND,
        "./scripts/confinement-tests.sh --here",
        "wire contract: the one command a granted run executes"
    );
}

#[test]
fn the_response_states_each_outcome() {
    let started = run_response(ConfinementRun::Started {
        task_id: "t1".into(),
        timeout_secs: 3600,
    })
    .unwrap();
    assert_eq!(started["state"], "started");
    assert_eq!(started["task_id"], "t1");
    let asked = run_response(ConfinementRun::Asked {
        card_id: "c1".into(),
    })
    .unwrap();
    assert_eq!(
        (asked["state"].as_str(), asked["card_id"].as_str()),
        (Some("asked"), Some("c1"))
    );
    let closed = run_response(ConfinementRun::Closed {
        card_id: "c1".into(),
    })
    .unwrap();
    assert_eq!(closed["state"], "closed");
    let unpermitted = run_response(ConfinementRun::Unpermitted).unwrap();
    assert_eq!(unpermitted["state"], "unpermitted");
    let msg = unpermitted["message"].as_str().unwrap();
    assert!(msg.contains("lucidos confinement-tests run"), "{msg}");
    let refused = run_response(ConfinementRun::Refused {
        reason: "no worktree".into(),
    });
    assert_eq!(refused, Err((StatusCode::CONFLICT, "no worktree".into())));
}
