use crate::support::{base_url, unique_marker, user_client};
use serde_json::json;

fn missing_id() -> String {
    unique_marker("e2e-absent-mcp")
}

#[tokio::test]
async fn delete_on_an_unknown_server_is_404() {
    let client = user_client().await;
    let api = base_url();
    let id = missing_id();

    let resp = client
        .delete(format!("{}/api/v1/mcp/servers/{}", api, id))
        .send()
        .await
        .expect("delete failed");

    assert_eq!(
        resp.status(),
        404,
        "removing a server that does not exist must not report success"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap_or_default().contains(&id),
        "the error should name the id: {body}"
    );
}

#[tokio::test]
async fn start_and_stop_on_an_unknown_server_are_404() {
    let client = user_client().await;
    let api = base_url();
    let id = missing_id();

    for verb in ["start", "stop"] {
        let resp = client
            .post(format!("{}/api/v1/mcp/servers/{}/{}", api, id, verb))
            .json(&json!({}))
            .send()
            .await
            .unwrap_or_else(|e| panic!("{verb} failed: {e}"));
        assert_eq!(resp.status(), 404, "{verb} on an unknown id");
    }
}

#[tokio::test]
async fn switching_a_tool_on_an_unknown_server_is_404() {
    let client = user_client().await;
    let api = base_url();
    let id = missing_id();

    let resp = client
        .patch(format!("{}/api/v1/mcp/servers/{}/disabled-tools", api, id))
        .json(&json!({ "wire_name": "mcp__x__y", "disabled": true }))
        .send()
        .await
        .expect("patch failed");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn listing_servers_carries_totals_and_the_resolved_window() {
    let client = user_client().await;
    let api = base_url();

    let resp = client
        .get(format!("{}/api/v1/mcp/servers", api))
        .send()
        .await
        .expect("list failed");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    assert!(body["servers"].is_array(), "servers array: {body}");
    let totals = &body["totals"];
    for key in [
        "servers",
        "running_servers",
        "tools",
        "chars",
        "tokens",
        "stopped_tools",
        "stopped_chars",
        "stopped_tokens",
        "disabled_tools",
        "disabled_chars",
        "disabled_tokens",
    ] {
        assert!(
            totals[key].is_u64(),
            "totals.{key} should be a number: {totals}"
        );
    }

    assert!(body["model"].is_string(), "model: {body}");
    assert!(
        body["context_window"].as_u64().unwrap_or(0) > 0,
        "context_window: {body}"
    );

    let budget = &body["budget"];
    let request_chars = budget["request_chars"].as_u64().unwrap_or(0);
    let ceiling = budget["mcp_ceiling_chars"].as_u64().unwrap_or(0);
    assert!(request_chars > 0, "budget.request_chars: {budget}");
    assert!(
        ceiling > 0 && ceiling < request_chars,
        "budget.mcp_ceiling_chars: {budget}"
    );
    for key in ["sent_tools", "sent_chars", "sent_tokens"] {
        assert!(budget[key].is_u64(), "budget.{key}: {budget}");
    }
    assert!(
        budget["sent_chars"].as_u64().unwrap() <= ceiling,
        "a request never carries more than the ceiling: {budget}"
    );
    assert!(budget["dropped"].is_array(), "budget.dropped: {budget}");
    assert!(
        budget["warning"].is_null() || budget["warning"].is_string(),
        "budget.warning: {budget}"
    );

    let servers = body["servers"].as_array().unwrap();
    assert_eq!(totals["servers"].as_u64().unwrap(), servers.len() as u64);

    for server in servers {
        assert!(
            ["live", "cache", "never-observed"]
                .contains(&server["tools_source"].as_str().unwrap_or_default()),
            "unexpected tools_source: {server}"
        );
        assert!(server["dispatchable"].is_boolean());
    }
}

#[tokio::test]
async fn mcp_allowed_tools_round_trips_over_http() {
    let client = user_client().await;
    let api = base_url();
    let url = format!("{}/api/v1/mcp-allowed-tools", api);

    let original: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("get failed")
        .json()
        .await
        .unwrap();
    let before = original["contents"].as_str().expect("contents").to_string();

    let pattern = format!("Mcp({}:*)", unique_marker("e2e-mcp"));
    let next = format!("{before}{pattern}\n");
    let resp = client
        .put(&url)
        .json(&json!({ "contents": next }))
        .send()
        .await
        .expect("put failed");
    assert_eq!(resp.status(), 204);

    let after: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .expect("get failed")
        .json()
        .await
        .unwrap();
    assert!(
        after["contents"].as_str().unwrap().contains(&pattern),
        "the written pattern should read back"
    );

    client
        .put(&url)
        .json(&json!({ "contents": before }))
        .send()
        .await
        .expect("restore failed");
}

use crate::support::{db_url, poll_thread_summary_by_marker};

const STUB_SERVER: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"stub","version":"1"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"alpha","description":"Says hello.","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"stub ran"}]}}\n' "$id"
      ;;
  esac
done
"#;

async fn register_stub(pool: &sqlx::PgPool) -> (String, tempfile::TempDir) {
    let id = unique_marker("e2e-stub");
    let dir = tempfile::TempDir::new().expect("tempdir");
    let script = dir.path().join("mcp-stub.sh");
    std::fs::write(&script, STUB_SERVER).expect("write the stub server");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    sqlx::query(
        "INSERT INTO mcp_servers (id, name, command, args, env) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&id)
    .bind("E2E stub")
    .bind(script.display().to_string())
    .bind(json!([]))
    .bind(json!({}))
    .execute(pool)
    .await
    .expect("register the stub server");
    (id, dir)
}

async fn ended_turns(pool: &sqlx::PgPool, thread_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type IN \
         ('ResponseGenerated', 'ResponseCanceled', 'ResponseAborted', 'ResponseFailed')",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn await_ended_turns(pool: &sqlx::PgPool, thread_id: uuid::Uuid, turns: i64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while ended_turns(pool, thread_id).await < turns {
        assert!(
            std::time::Instant::now() < deadline,
            "turn {turns} on thread {thread_id} never ended"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn open_thread(client: &reqwest::Client, pool: &sqlx::PgPool) -> uuid::Uuid {
    let marker = unique_marker("e2e-mcp-thread");
    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({ "message": format!("noop {marker}"), "mode": "human" }))
        .send()
        .await
        .expect("chat failed");
    assert_eq!(resp.status(), 200);
    let thread_id = poll_thread_summary_by_marker(pool, &marker, 15)
        .await
        .thread_id;
    await_ended_turns(pool, thread_id, 1).await;
    thread_id
}

async fn tools_of_next_turn(
    client: &reqwest::Client,
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Vec<String> {
    let ended = ended_turns(pool, thread_id).await;
    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({ "message": "noop follow-up", "mode": "human", "thread_id": thread_id.to_string() }))
        .send()
        .await
        .expect("follow-up failed");
    assert_eq!(resp.status(), 200);
    await_ended_turns(pool, thread_id, ended + 1).await;
    let (payload,): (serde_json::Value,) = sqlx::query_as(
        "SELECT payload FROM events WHERE thread_id = $1 AND event_type = 'ContextCaptured' \
         AND payload->>'producer' = 'main_llm' ORDER BY created DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap();
    payload["tools"]
        .as_array()
        .expect("tools on ContextCaptured")
        .iter()
        .filter_map(|t| t.as_str().map(str::to_string))
        .collect()
}

async fn enabled_of(pool: &sqlx::PgPool, thread_id: uuid::Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT enabled_mcp_servers FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn enable_and_disable_name_the_unknown_server_or_thread() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let thread_id = open_thread(&client, &pool).await;
    let id = missing_id();

    for verb in ["enable", "disable"] {
        let resp = client
            .post(format!(
                "{}/api/v1/threads/{}/mcp-servers/{}/{}",
                base_url(),
                thread_id,
                id,
                verb
            ))
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "{verb} on an unknown server");

        let resp = client
            .post(format!(
                "{}/api/v1/threads/{}/mcp-servers/{}/{}",
                base_url(),
                uuid::Uuid::new_v4(),
                id,
                verb
            ))
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "{verb} on an unknown thread");
    }
}

#[tokio::test]
async fn a_server_enabled_in_one_thread_reaches_only_that_threads_turns() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let (id, _dir) = register_stub(&pool).await;
    let user = open_thread(&client, &pool).await;
    let bystander = open_thread(&client, &pool).await;
    let route = |thread: uuid::Uuid, verb: &str| {
        format!(
            "{}/api/v1/threads/{}/mcp-servers/{}/{}",
            base_url(),
            thread,
            id,
            verb
        )
    };

    let enabled: serde_json::Value = client
        .post(route(user, "enable"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(enabled["enabled"], true, "{enabled}");
    assert_eq!(enabled["tool_count"], 1, "{enabled}");
    assert_eq!(enabled_of(&pool, user).await, std::slice::from_ref(&id));

    let wire = format!("mcp__{id}__alpha");
    assert!(tools_of_next_turn(&client, &pool, user)
        .await
        .contains(&wire));
    assert!(!tools_of_next_turn(&client, &pool, bystander)
        .await
        .contains(&wire));

    let disabled: serde_json::Value = client
        .post(route(user, "disable"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(disabled["was_enabled"], true, "{disabled}");
    assert!(enabled_of(&pool, user).await.is_empty());
    let listed: serde_json::Value = client
        .get(format!("{}/api/v1/mcp/servers", base_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = listed["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id.as_str())
        .cloned();
    assert_eq!(
        row.expect("the stub is listed")["running"],
        true,
        "a disable leaves the process running"
    );

    for thread in [user, bystander] {
        let resp = client
            .post(route(thread, "enable"))
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
    let resp = client
        .post(format!("{}/api/v1/mcp/servers/{}/stop", base_url(), id))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(enabled_of(&pool, user).await.is_empty());
    assert!(enabled_of(&pool, bystander).await.is_empty());
    let stopped: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type = 'McpServerStopped' AND aggregate_id = $1",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stopped, 1);

    let resp = client
        .delete(format!("{}/api/v1/mcp/servers/{}", base_url(), id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
}

async fn trigger_named(client: &reqwest::Client, name: &str) -> (String, Vec<String>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let listed: serde_json::Value = client
            .get(format!("{}/api/v1/triggers", base_url()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let found = listed["triggers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .cloned();
        if let Some(t) = found {
            let servers = t["mcp_servers"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            return (t["id"].as_str().unwrap().to_string(), servers);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "trigger {name} never listed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_trigger_lists_its_mcp_servers_through_the_api() {
    let client = user_client().await;
    let name = unique_marker("e2e-trigger-mcp");
    let created: serde_json::Value = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&json!({
            "name": name,
            "run": { "type": "intent", "intent": "noop" },
            "cron_expressions": ["0 0 8 * * *"],
            "mcp_servers": [" slack ", "slack", "jira"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["success"], true, "{created}");

    let (trigger_id, servers) = trigger_named(&client, &name).await;
    assert_eq!(servers, ["slack", "jira"]);

    for (update, want) in [(json!(["github"]), vec!["github"]), (json!([]), vec![])] {
        let res: serde_json::Value = client
            .put(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
            .json(&json!({ "mcp_servers": update }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(res["success"], true, "{res}");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let (_, servers) = trigger_named(&client, &name).await;
            if servers == want {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "mcp_servers stayed {servers:?}, wanted {want:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .send()
        .await;
}

#[tokio::test]
async fn a_trigger_listing_a_server_has_its_tools_from_the_first_round() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let (id, _dir) = register_stub(&pool).await;
    for action in ["start", "stop"] {
        let response = client
            .post(format!("{}/api/v1/mcp/servers/{id}/{action}", base_url()))
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "stub {action}: {}",
            response.status()
        );
    }
    let name = unique_marker("e2e-trigger-mcp-fire");
    let created: serde_json::Value = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&json!({
            "name": name,
            "run": { "type": "intent", "intent": "noop" },
            "cron_expressions": ["0 0 8 1 1 *"],
            "mcp_servers": [id],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["success"], true, "{created}");
    let (trigger_id, servers) = trigger_named(&client, &name).await;
    assert_eq!(servers, std::slice::from_ref(&id));

    let resp = client
        .post(format!(
            "{}/api/v1/triggers/run?id={}",
            base_url(),
            trigger_id
        ))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "run: {}", resp.status());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let (thread_id, first_tools) = loop {
        let row: Option<(uuid::Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT e.thread_id, e.payload->'tools' FROM events e \
             JOIN thread_summaries ts ON ts.thread_id = e.thread_id \
             WHERE ts.trigger_id = $1 AND e.event_type = 'ContextCaptured' \
               AND e.payload->>'producer' = 'main_llm' \
             ORDER BY e.created LIMIT 1",
        )
        .bind(&trigger_id)
        .fetch_optional(&pool)
        .await
        .unwrap();
        if let Some(found) = row {
            break found;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the fire's first capture never landed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    };
    let wire = format!("mcp__{id}__alpha");
    assert!(
        first_tools
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == wire.as_str()),
        "round 1 must carry {wire}: {first_tools}"
    );
    let by: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'by' FROM events WHERE thread_id = $1 AND event_type = 'ThreadMcpServerEnabled'",
    )
    .bind(thread_id)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(by.as_deref(), Some("trigger"));

    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .send()
        .await;
    let _ = client
        .delete(format!("{}/api/v1/mcp/servers/{}", base_url(), id))
        .send()
        .await;
}
