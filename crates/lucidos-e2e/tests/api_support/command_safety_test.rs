use crate::support::{base_url, local_process_client, unique_marker, user_client};
use lucidos_engine::core::prefs;
use serde_json::json;

#[tokio::test]
async fn standing_deny_settings_control_the_live_hook_check() {
    let owner = user_client().await;
    let settings = format!("{}/api/v1/agent-denied-commands", base_url());
    let check = format!("{}/api/v1/internal/command-deny-check", base_url());
    let program = unique_marker("e2e-denied");
    let rule = format!("Bash({program}:*)");
    let original: serde_json::Value = owner
        .get(&settings)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let original = original["contents"].as_str().unwrap();
    let command = json!({"command": format!("'{program}' --help")});
    let set = owner
        .put(&settings)
        .json(&json!({"contents": format!("{original}\n{rule}")}))
        .send()
        .await
        .unwrap();
    let refused = local_process_client()
        .put(&settings)
        .json(&json!({"contents": original}))
        .send()
        .await
        .unwrap();
    let blocked = owner.post(&check).json(&command).send().await.unwrap();
    let restore = owner
        .put(&settings)
        .json(&json!({"contents": original}))
        .send()
        .await
        .unwrap();
    let cleared = owner.post(&check).json(&command).send().await.unwrap();

    assert_eq!(set.status().as_u16(), 204);
    assert_eq!(refused.status().as_u16(), 403);
    assert_eq!(restore.status().as_u16(), 204);
    let refusal: String = blocked.error_for_status().unwrap().json().await.unwrap();
    assert!(refusal.contains(&rule), "{refusal}");
    let refusal: Option<String> = cleared.error_for_status().unwrap().json().await.unwrap();
    assert_eq!(refusal, None);
}

async fn set_timezone_utc(client: &reqwest::Client) {
    let _ = client
        .put(format!(
            "{}/api/v1/preferences?key={}",
            base_url(),
            prefs::TIMEZONE.key()
        ))
        .json(&json!({ "value": "UTC" }))
        .send()
        .await;
}

async fn create_trigger_with_grant(client: &reqwest::Client, name: &str, grant: &[&str]) {
    let body = json!({
        "name": name,
        "run": { "type": "intent", "intent": "noop" },
        "cron_expressions": ["0 0 8 * * *"],
        "side_effect_grant": grant,
    });
    let resp = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&body)
        .send()
        .await
        .expect("POST /triggers failed");
    let res: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(
        res["success"], true,
        "Trigger create must succeed: {:?}",
        res
    );
}

async fn delete_trigger(client: &reqwest::Client, trigger_id: &str) {
    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .send()
        .await;
}

fn grant_of(trigger: &serde_json::Value) -> Vec<String> {
    let mut v: Vec<String> = trigger["side_effect_grant"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

async fn poll_trigger_grant(
    client: &reqwest::Client,
    name: &str,
    expected_sorted: &[&str],
) -> String {
    let mut want: Vec<String> = expected_sorted.iter().map(|s| s.to_string()).collect();
    want.sort();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let listed: serde_json::Value = client
            .get(format!("{}/api/v1/triggers", base_url()))
            .send()
            .await
            .expect("GET /triggers failed")
            .json()
            .await
            .expect("Invalid JSON");
        let found = listed["triggers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"].as_str() == Some(name))
            .cloned();
        if let Some(ref t) = found {
            if grant_of(t) == want {
                return t["id"].as_str().unwrap().to_string();
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Trigger '{}' grant did not become {:?} within 5s (last saw {:?})",
            name,
            want,
            found.as_ref().map(grant_of),
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn trigger_create_and_update_refuse_new_category_grants() {
    let client = user_client().await;
    set_timezone_utc(&client).await;
    let name = unique_marker("e2e-grant");
    let response: serde_json::Value = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&json!({
            "name": name,
            "run": { "type": "intent", "intent": "noop" },
            "cron_expressions": ["0 0 8 * * *"],
            "side_effect_grant": ["email"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["success"], false, "{response}");
    assert!(response["error"]
        .as_str()
        .unwrap()
        .contains("Category grants are retired"));

    create_trigger_with_grant(&client, &name, &[]).await;
    let trigger_id = poll_trigger_grant(&client, &name, &[]).await;
    let response: serde_json::Value = client
        .put(format!("{}/api/v1/triggers?id={trigger_id}", base_url()))
        .json(&json!({ "side_effect_grant": ["email"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["success"], false, "{response}");
    poll_trigger_grant(&client, &name, &[]).await;
    delete_trigger(&client, &trigger_id).await;
}

async fn app_frame_trigger_write(
    client: &reqwest::Client,
    method: reqwest::Method,
    query: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    client
        .request(method, format!("{}/api/v1/triggers{query}", base_url()))
        .header("x-lucidos-app-id", "habit-tracker")
        .json(&body)
        .send()
        .await
        .expect("trigger write failed")
        .json()
        .await
        .expect("Invalid JSON")
}

#[tokio::test]
async fn an_app_frame_cannot_widen_a_side_effect_grant() {
    let client = user_client().await;
    set_timezone_utc(&client).await;
    let name = unique_marker("e2e-app-grant");

    let refused = app_frame_trigger_write(
        &client,
        reqwest::Method::POST,
        "",
        json!({
            "name": name,
            "run": { "type": "intent", "intent": "noop" },
            "cron_expressions": ["0 0 8 * * *"],
            "side_effect_grant": ["email"],
        }),
    )
    .await;
    assert!(
        refused["error"]
            .as_str()
            .is_some_and(|e| e.contains("Only the owner")),
        "{refused:?}"
    );

    create_trigger_with_grant(&client, &name, &[]).await;
    let trigger_id = poll_trigger_grant(&client, &name, &[]).await;
    let query = format!("?id={trigger_id}");

    let widened = app_frame_trigger_write(
        &client,
        reqwest::Method::PUT,
        &query,
        json!({ "side_effect_grant": ["email", "external_api"] }),
    )
    .await;
    assert_eq!(widened["success"], false, "{widened:?}");

    let narrowed = app_frame_trigger_write(
        &client,
        reqwest::Method::PUT,
        &query,
        json!({ "side_effect_grant": [] }),
    )
    .await;
    assert_eq!(narrowed["success"], true, "{narrowed:?}");
    poll_trigger_grant(&client, &name, &[]).await;

    delete_trigger(&client, &trigger_id).await;
}

async fn poll_trigger_field(
    client: &reqwest::Client,
    name: &str,
    field: &str,
    expected: &serde_json::Value,
) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let listed: serde_json::Value = client
            .get(format!("{}/api/v1/triggers", base_url()))
            .send()
            .await
            .expect("GET /triggers failed")
            .json()
            .await
            .expect("Invalid JSON");
        let found = listed["triggers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"].as_str() == Some(name))
            .cloned();
        if let Some(ref t) = found {
            if &t[field] == expected {
                return t["id"].as_str().unwrap().to_string();
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Trigger '{name}' {field} did not become {expected} within 5s \
             (last saw {found:?})",
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn trigger_coding_agent_grant_round_trips() {
    let client = user_client().await;
    set_timezone_utc(&client).await;
    let name = unique_marker("e2e-coding-agent-grant");

    let created: serde_json::Value = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&json!({
            "name": name,
            "run": { "type": "intent", "intent": "noop" },
            "cron_expressions": ["0 0 8 * * *"],
        }))
        .send()
        .await
        .expect("POST /triggers failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert_eq!(created["success"], true, "{created:?}");
    let field = "coding_agent_grant";
    let trigger_id = poll_trigger_field(&client, &name, field, &json!(false)).await;

    for grant in [true, false] {
        let res: serde_json::Value = client
            .put(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
            .json(&json!({ field: grant }))
            .send()
            .await
            .expect("PUT /triggers failed")
            .json()
            .await
            .expect("Invalid JSON");
        assert_eq!(res["success"], true, "{res:?}");
        poll_trigger_field(&client, &name, field, &json!(grant)).await;
    }

    delete_trigger(&client, &trigger_id).await;
}

#[tokio::test]
async fn trigger_create_rejects_unknown_side_effect_category() {
    let client = user_client().await;
    set_timezone_utc(&client).await;

    let body = json!({
        "name": unique_marker("e2e-bad-grant"),
        "run": { "type": "intent", "intent": "noop" },
        "cron_expressions": ["0 0 8 * * *"],
        "side_effect_grant": ["yolo"],
    });
    let resp = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&body)
        .send()
        .await
        .expect("POST /triggers failed");
    assert_eq!(
        resp.status().as_u16(),
        422,
        "Unknown side-effect category must be rejected at the serde boundary (422)"
    );
}

#[tokio::test]
async fn trigger_update_rejects_unknown_side_effect_category() {
    let client = user_client().await;
    set_timezone_utc(&client).await;
    let name = unique_marker("e2e-bad-update");

    create_trigger_with_grant(&client, &name, &[]).await;
    let trigger_id = poll_trigger_grant(&client, &name, &[]).await;

    let resp = client
        .put(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .json(&json!({ "side_effect_grant": ["not_a_category"] }))
        .send()
        .await
        .expect("PUT /triggers failed");
    assert_eq!(
        resp.status().as_u16(),
        422,
        "Unknown side-effect category on update must be rejected (422)"
    );

    poll_trigger_grant(&client, &name, &[]).await;
    delete_trigger(&client, &trigger_id).await;
}

#[tokio::test]
async fn command_checkpoint_undo_unknown_id_returns_error() {
    let client = user_client().await;
    let unknown_id = uuid::Uuid::new_v4().to_string();

    let resp = client
        .post(format!("{}/api/v1/command-checkpoint/undo", base_url()))
        .json(&json!({ "checkpoint_id": unknown_id }))
        .send()
        .await
        .expect("POST /command-checkpoint/undo failed");

    assert_eq!(
        resp.status().as_u16(),
        400,
        "Unknown checkpoint id must return 400 Bad Request"
    );
    let body = resp.text().await.expect("Failed to read body");
    assert!(
        body.starts_with("[ERROR]"),
        "Error body must use the [ERROR] prefix, got: {body:?}"
    );
    assert!(
        body.contains("no command checkpoint"),
        "Error body should name the missing checkpoint, got: {body:?}"
    );
}

#[tokio::test]
async fn command_checkpoint_undo_missing_id_is_rejected() {
    let client = user_client().await;
    let resp = client
        .post(format!("{}/api/v1/command-checkpoint/undo", base_url()))
        .json(&json!({}))
        .send()
        .await
        .expect("POST /command-checkpoint/undo failed");
    let status = resp.status().as_u16();
    assert!(
        (400..500).contains(&status),
        "Missing checkpoint_id should return 4xx, got {status}"
    );
}

#[tokio::test]
async fn command_checkpoint_diff_rejects_a_non_uuid_id() {
    let client = user_client().await;
    for id in ["../../heads/main", "not-a-uuid", ""] {
        let resp = client
            .get(format!("{}/api/v1/command-checkpoint/diff", base_url()))
            .query(&[("checkpoint_id", id)])
            .send()
            .await
            .expect("GET /command-checkpoint/diff failed");
        assert_eq!(
            resp.status().as_u16(),
            400,
            "checkpoint_id {id:?} must be rejected as malformed"
        );
    }
}

#[tokio::test]
async fn command_checkpoint_diff_unknown_id_is_not_found() {
    let client = user_client().await;
    let resp = client
        .get(format!("{}/api/v1/command-checkpoint/diff", base_url()))
        .query(&[("checkpoint_id", uuid::Uuid::new_v4().to_string())])
        .send()
        .await
        .expect("GET /command-checkpoint/diff failed");
    assert_eq!(resp.status().as_u16(), 404);
}

async fn get_preference(client: &reqwest::Client, key: &str) -> Option<String> {
    let resp = client
        .get(format!("{}/api/v1/preferences?key={key}", base_url()))
        .send()
        .await
        .ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("value")?.as_str().map(str::to_string)
}

async fn set_preference(client: &reqwest::Client, key: &str, value: Option<&str>) {
    let body = match value {
        Some(v) => json!({ "value": v }),
        None => json!({ "value": "" }),
    };
    let resp = client
        .put(format!("{}/api/v1/preferences?key={key}", base_url()))
        .json(&body)
        .send()
        .await
        .expect("preference write failed");
    assert!(
        resp.status().is_success(),
        "setting {key} should succeed, got {}",
        resp.status()
    );
}

fn checkpoint_ref_set() -> std::collections::BTreeSet<String> {
    let out = crate::support::git_in(
        &crate::support::workspace_path(),
        &["for-each-ref", "--format=%(refname)", "refs/lucidos/"],
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn a_command_destroying_only_ignored_content_leaves_no_undo_card() {
    let pool = sqlx::PgPool::connect(&crate::support::db_url())
        .await
        .expect("connect to the e2e workspace database");
    let client = crate::support::user_client().await;

    let guard = prefs::COMMAND_GUARD.key();
    let judge = prefs::COMMAND_GUARD_JUDGE.key();
    let prior_guard = get_preference(&client, guard).await;
    let prior_judge = get_preference(&client, judge).await;
    set_preference(&client, guard, Some("true")).await;
    set_preference(&client, judge, Some("false")).await;

    let scratch_rel = format!(".lucidos/tmp/e2e-ckpt-{}", uuid::Uuid::new_v4().simple());
    let scratch = crate::support::workspace_path().join(&scratch_rel);
    std::fs::create_dir_all(&scratch).expect("create scratch dir");
    std::fs::write(scratch.join("f.txt"), "scratch").expect("write scratch file");

    let _tree_quiet = crate::support::workspace_tree_lock().write().await;

    let refs_before = checkpoint_ref_set();
    let marker = crate::support::unique_marker("api-command-guard");
    let code = format!("import shutil; shutil.rmtree('{scratch_rel}')");
    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({
            "message": format!("{marker} MOCK_RUN_PYTHON:{code}"),
            "mode": "human",
        }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200, "chat/stream should accept the message");

    let thread_id = crate::support::poll_thread_summary_by_marker(&pool, &marker, 25)
        .await
        .thread_id;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let done: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
               AND event_type IN ('ResponseGenerated', 'ResponseFailed', 'ResponseAborted')",
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("DB query failed");
        if done > 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the guarded turn never terminated"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    let tool_results: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
           AND event_type = 'ToolResult'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("DB query failed");
    let checkpoints: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
           AND event_type = 'CommandCheckpointed'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("DB query failed");

    set_preference(&client, guard, prior_guard.as_deref()).await;
    set_preference(&client, judge, prior_judge.as_deref()).await;

    assert_eq!(
        tool_results, 1,
        "the scripted run_python should have produced exactly one ToolResult"
    );
    assert!(
        !scratch.exists(),
        "the command should have destroyed its scratch dir at {scratch_rel}"
    );
    assert_eq!(
        checkpoints, 0,
        "a command whose destruction was gitignored must leave no undo card"
    );
    let leaked: Vec<String> = checkpoint_ref_set()
        .difference(&refs_before)
        .cloned()
        .collect();
    assert!(
        leaked.is_empty(),
        "the empty-diff path should have deleted both checkpoint refs, but these survived: {leaked:?}"
    );
}

async fn poll_thread_event(
    pool: &sqlx::PgPool,
    thread_id: &str,
    event_types: &[&str],
    secs: u64,
) -> Option<serde_json::Value> {
    let types: Vec<String> = event_types.iter().map(|t| t.to_string()).collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        let payload: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT payload FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
               AND event_type = ANY($2) ORDER BY sequence LIMIT 1",
        )
        .bind(thread_id)
        .bind(&types)
        .fetch_optional(pool)
        .await
        .expect("DB query failed");
        if payload.is_some() || std::time::Instant::now() >= deadline {
            return payload;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn mcp_server_rows(pool: &sqlx::PgPool, id: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM mcp_servers WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("DB query failed")
}

#[tokio::test]
async fn an_mcp_server_setup_asks_before_it_spawns_anything() {
    let pool = sqlx::PgPool::connect(&crate::support::db_url())
        .await
        .expect("connect to the e2e workspace database");
    let client = crate::support::user_client().await;

    let server_id = format!("e2e-guard-{}", uuid::Uuid::new_v4().simple());
    let secret_file = format!("{server_id}-missing");
    let secret = lucidos_engine::core::secret_store::key_store::default_key_dir()
        .expect("the e2e host has a HOME")
        .join(&secret_file);
    let args = json!({
        "action": "setup",
        "id": server_id,
        "name": "E2E guard",
        "command": "cat",
        "args": [secret],
    });
    let marker = unique_marker("api-mcp-setup-guard");
    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({
            "message": format!("{marker} MOCK_TOOL_CALL: mcp {args}"),
            "mode": "human",
        }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200, "chat/stream should accept the message");
    let thread_id = crate::support::poll_thread_summary_by_marker(&pool, &marker, 25)
        .await
        .thread_id
        .to_string();

    let card = poll_thread_event(&pool, &thread_id, &["CommandPermissionRequested"], 30)
        .await
        .expect("the MCP server setup should have raised a permission card");
    assert_eq!(
        mcp_server_rows(&pool, &server_id).await,
        0,
        "nothing may register before an answer"
    );
    let trace = card["decision_trace"]
        .as_str()
        .expect("the card carries its guard trace");
    assert!(trace.contains("layer=static-secret-path"), "{trace}");
    assert!(trace.contains("lane=IrreversibleDanger"), "{trace}");
    assert!(
        !trace.contains(&secret_file),
        "the trace carries no command text"
    );
    let command = card["command"].as_str().unwrap_or_default();
    assert!(
        command.contains("cat ") && command.contains(&secret_file),
        "the card should show the command line the server would run: {command}"
    );
    let summary = card["summary"].as_str().unwrap_or_default();
    assert!(
        summary.starts_with(lucidos_engine::engine::command_permission::MCP_SETUP_SUMMARY),
        "the card should say an allowed server keeps running this: {summary}"
    );

    let cancel = client
        .post(format!(
            "{}/api/v1/chat/cancel?thread_id={thread_id}",
            base_url()
        ))
        .send()
        .await
        .expect("cancel request failed");
    assert!(cancel.status().is_success(), "cancel should succeed");
    let ended = poll_thread_event(
        &pool,
        &thread_id,
        &[
            "ResponseGenerated",
            "ResponseFailed",
            "ResponseAborted",
            "ResponseCanceled",
        ],
        30,
    )
    .await;
    assert!(ended.is_some(), "the canceled turn never ended");
    assert_eq!(
        mcp_server_rows(&pool, &server_id).await,
        0,
        "a canceled card must leave no MCP server behind"
    );
}
