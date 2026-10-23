//! API e2e for the *intelligence ladder* (ADR 0500, ADR 0525): the route that
//! serves each *ladder surface*, and the refusal of an agent's pick off it.

use crate::support::{base_url, local_process_client, user_client};
use serde_json::json;

async fn get_ladder(query: &str) -> serde_json::Value {
    get_ladder_at("intelligence-ladder", query).await
}

async fn get_ladder_at(route: &str, query: &str) -> serde_json::Value {
    let resp = user_client()
        .await
        .get(format!("{}/api/v1/{route}{query}", base_url()))
        .send()
        .await
        .expect("ladder request failed");
    assert_eq!(resp.status().as_u16(), 200, "the ladder route answers 200");
    resp.json().await.expect("ladder response was not JSON")
}

/// The shipped Claude Code ladder serves every shipped pair, the ones that
/// are on matching the shipped list, and the default rung reads 1.0×.
#[tokio::test]
async fn the_route_serves_the_claude_code_ladder_with_its_default_rung() {
    let ladder = get_ladder("?surface=claude-code").await;
    assert_eq!(ladder["surface"], "claude-code", "{ladder}");
    assert_eq!(ladder["coding_agent"], "claude-code", "{ladder}");
    assert_eq!(ladder["customised"], false, "{ladder}");
    assert_eq!(ladder["climbs"], true, "{ladder}");
    assert_eq!(
        ladder["default_rung"],
        json!({"model": "claude-opus-5-5", "effort": "medium"}),
        "{ladder}"
    );
    let rungs = ladder["rungs"].as_array().expect("rungs");
    let on: Vec<serde_json::Value> = rungs
        .iter()
        .filter(|r| r["on"] == true)
        .map(|r| json!({"model": r["model"], "effort": r["effort"]}))
        .collect();
    assert_eq!(
        serde_json::Value::from(on),
        ladder["shipped_rungs"],
        "{ladder}"
    );
    assert!(
        rungs.iter().any(|r| r["on"] == false),
        "off pairs are served too"
    );
    assert!(
        rungs.iter().all(|r| r.get("score").is_none()),
        "no third-party figure is served: {ladder}"
    );
    let default = rungs
        .iter()
        .find(|r| r["is_default"] == true)
        .expect("one rung is the default");
    assert_eq!(default["cost"]["label"], "1.0×", "{default}");
    assert_eq!(default["time"]["label"], "1.0×", "{default}");
}

/// Codex has no default, so its bands compare against Claude Code's default.
#[tokio::test]
async fn the_codex_ladder_compares_against_the_claude_code_default() {
    let ladder = get_ladder("?surface=codex").await;
    assert_eq!(ladder["default_rung"], serde_json::Value::Null, "{ladder}");
    assert_eq!(
        ladder["reference"],
        json!({"model": "claude-opus-5-5", "effort": "medium"}),
        "{ladder}"
    );
}

/// The old route and its `coding_agent` parameter still serve the same
/// ladder.
#[tokio::test]
async fn the_coding_agent_route_still_serves_the_ladder() {
    let old = get_ladder_at("coding-agent/intelligence-ladder", "?coding_agent=codex").await;
    assert_eq!(old, get_ladder("?surface=codex").await);
    assert_eq!(get_ladder("?coding_agent=codex").await["surface"], "codex");
}

/// The chat ladder names no backend. Its bands count from the account's chat
/// default, and every served pair has one row.
#[tokio::test]
async fn the_route_serves_the_chat_ladder() {
    let ladder = get_ladder("?surface=chat").await;
    assert_eq!(ladder["surface"], "chat", "{ladder}");
    assert!(ladder.get("coding_agent").is_none(), "{ladder}");
    let rungs = ladder["rungs"].as_array().expect("rungs");
    assert!(rungs.iter().any(|r| r["on"] == true), "{ladder}");
    let mut seen = std::collections::HashSet::new();
    for r in rungs {
        assert!(
            seen.insert((r["model"].to_string(), r["effort"].to_string())),
            "one row per model and effort: {ladder}"
        );
    }
    assert_eq!(ladder["reference"], ladder["default_rung"], "{ladder}");
}

const TRIGGER_NAME: &str = "ladder e2e off-ladder trigger";

/// The owner may pin a trigger to a pair off the chat ladder.
#[tokio::test]
async fn a_persons_off_ladder_trigger_model_is_not_refused() {
    let client = user_client().await;
    let created: serde_json::Value = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&json!({
            "name": TRIGGER_NAME,
            "run": {"type": "intent", "intent": "noop"},
            "cron_expressions": ["0 0 3 1 1 *"],
            "model": "claude-sonnet-5-5",
            "reasoning_effort": "low",
        }))
        .send()
        .await
        .expect("POST /triggers failed")
        .json()
        .await
        .expect("trigger response was not JSON");
    assert_eq!(created["success"], true, "{created}");
    let list: serde_json::Value = client
        .get(format!("{}/api/v1/triggers", base_url()))
        .send()
        .await
        .expect("GET /triggers failed")
        .json()
        .await
        .expect("trigger list was not JSON");
    let id = list["triggers"]
        .as_array()
        .and_then(|all| all.iter().find(|t| t["name"] == TRIGGER_NAME))
        .and_then(|t| t["id"].as_str())
        .unwrap_or_else(|| panic!("the created trigger is listed: {list}"))
        .to_string();
    client
        .delete(format!("{}/api/v1/triggers?id={id}", base_url()))
        .send()
        .await
        .expect("DELETE /triggers failed");
}

/// A coding-agent spawn whose repo does not exist, so no request here starts
/// a thread: a refusal before the repo check is the ladder's, after it the
/// repo's.
fn off_ladder_spawn(mode: &str) -> serde_json::Value {
    json!({
        "message": "test",
        "mode": mode,
        "use_coding_agent": true,
        "cc_model": "claude-sonnet-5-5",
        "reasoning_effort": "low",
        "repo_id": "no-such-repo-for-the-ladder-e2e",
    })
}

async fn post_chat(client: &reqwest::Client, body: serde_json::Value) -> (u16, serde_json::Value) {
    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&body)
        .send()
        .await
        .expect("chat request failed");
    let status = resp.status().as_u16();
    let text = resp.text().await.expect("body");
    (
        status,
        serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)),
    )
}

/// An agent's pick off the ladder is refused, naming the ladder. An agent
/// request needs provenance, here a cross-workspace caller.
#[tokio::test]
async fn an_agents_off_ladder_pick_is_refused() {
    let mut spawn = off_ladder_spawn("agent");
    spawn["caller_workspace"] = "ladder-e2e-caller".into();
    let (status, body) = post_chat(&local_process_client(), spawn).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["reason"], "off-intelligence-ladder", "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("claude-opus-5-5=medium")),
        "the refusal names the rungs: {body}"
    );
}

/// The same pick from a person passes the ladder and stops at the repo.
#[tokio::test]
async fn a_persons_off_ladder_pick_is_not_refused_for_the_ladder() {
    let (status, body) = post_chat(&user_client().await, off_ladder_spawn("human")).await;
    assert_eq!(status, 400, "the missing repo refuses it: {body}");
    assert_ne!(body["reason"], "off-intelligence-ladder", "{body}");
}

/// An agent starting a chat thread on a pair off the Lucidos Agent's ladder is
/// refused, naming its rungs.
#[tokio::test]
async fn an_agents_off_ladder_chat_pick_is_refused() {
    let spawn = json!({
        "message": "test",
        "mode": "agent",
        "caller_workspace": "ladder-e2e-caller",
        "model": "claude-sonnet-5-5",
        "reasoning_effort": "low",
    });
    let (status, body) = post_chat(&local_process_client(), spawn).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["reason"], "off-intelligence-ladder", "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("Lucidos Agent intelligence ladder")),
        "the refusal names the Lucidos Agent's ladder: {body}"
    );
}

/// Copy from Lucidos Agent writes a coding agent's override, so it refuses the
/// chat surface itself, which would copy the ladder onto its own source. The
/// write itself is covered in `runtime::chat_ladder`, since a copy here would
/// change the Claude Code ladder the tests above read.
#[tokio::test]
async fn copy_from_chat_refuses_the_chat_surface() {
    let resp = user_client()
        .await
        .post(format!(
            "{}/api/v1/intelligence-ladder/copy-from-chat",
            base_url()
        ))
        .json(&json!({"surface": "chat"}))
        .send()
        .await
        .expect("copy request failed");
    assert_eq!(resp.status().as_u16(), 400);
    let body: serde_json::Value = resp.json().await.expect("JSON error body");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("onto itself")),
        "{body}"
    );
}

/// Sort by intelligence on a ladder that already climbs restates the shipped
/// rungs, so it stores nothing and the ladder stays uncustomised.
#[tokio::test]
async fn sorting_a_climbing_ladder_restates_it_and_stores_nothing() {
    let before = get_ladder("?surface=claude-code").await;
    let resp = user_client()
        .await
        .post(format!("{}/api/v1/intelligence-ladder/sort", base_url()))
        .json(&json!({"surface": "claude-code"}))
        .send()
        .await
        .expect("sort request failed");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("JSON body");
    assert_eq!(body["rungs"], before["shipped_rungs"], "{body}");
    let after = get_ladder("?surface=claude-code").await;
    assert_eq!(after["customised"], false, "{after}");
    assert_eq!(after["climbs"], true, "{after}");
}
