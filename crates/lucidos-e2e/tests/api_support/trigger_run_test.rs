use crate::support::{
    base_url, remove_data_fixtures, unique_marker, user_client, write_data_fixture,
};
use serde_json::json;
use std::time::{Duration, Instant};

const PROBE_SCRIPT: &str = r#"#!/usr/bin/env python3
print("ok")
"#;

const NEVER_SOON_CRON: &str = "0 0 3 1 1 *";

pub(crate) async fn create_trigger(
    client: &reqwest::Client,
    name: &str,
    slug: &str,
    run: serde_json::Value,
    body_extra: serde_json::Value,
) -> String {
    let mut body = json!({ "name": name, "slug": slug, "run": run });
    for (k, v) in body_extra.as_object().expect("extra is an object") {
        body[k] = v.clone();
    }

    let created: serde_json::Value = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&body)
        .send()
        .await
        .expect("POST /triggers failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert_eq!(created["success"], true, "create failed: {created}");

    find_trigger(client, name)
        .await
        .expect("created trigger is listed")["id"]
        .as_str()
        .expect("trigger id")
        .to_string()
}

fn unreachable_intent() -> serde_json::Value {
    json!({ "type": "intent", "intent": "never executed: this trigger's run is always refused" })
}

async fn find_trigger(client: &reqwest::Client, name: &str) -> Option<serde_json::Value> {
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/triggers", base_url()))
        .send()
        .await
        .expect("GET /triggers failed")
        .json()
        .await
        .expect("Invalid JSON");
    body["triggers"]
        .as_array()?
        .iter()
        .find(|t| t["name"] == name)
        .cloned()
}

async fn run_trigger(client: &reqwest::Client, id: &str) -> serde_json::Value {
    try_run_trigger(client, id).await.expect("run_trigger")
}

async fn try_run_trigger(client: &reqwest::Client, id: &str) -> Result<serde_json::Value, String> {
    let resp = client
        .post(format!("{}/api/v1/triggers/run?id={}", base_url(), id))
        .send()
        .await
        .map_err(|e| format!("POST /triggers/run failed: {e}"))?;
    resp.json()
        .await
        .map_err(|e| format!("invalid JSON from /triggers/run: {e}"))
}

pub(crate) async fn delete_trigger(client: &reqwest::Client, id: &str) {
    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), id))
        .send()
        .await;
}

type TurnTerminators = (Option<uuid::Uuid>, Vec<(String, Option<String>)>);

async fn read_turn_terminators(trigger_id: &str) -> Result<TurnTerminators, String> {
    let pool = sqlx::PgPool::connect(&crate::support::db_url())
        .await
        .map_err(|e| format!("connect to the e2e workspace database: {e}"))?;
    let result = async {
        let thread_id: Option<uuid::Uuid> = sqlx::query_scalar(
            "SELECT thread_id FROM events \
             WHERE event_type = 'TriggerStarted' \
               AND payload->'origin'->'reason'->>'trigger_id' = $1 \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(trigger_id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| format!("query TriggerStarted: {e}"))?;
        let Some(tid) = thread_id else {
            return Ok((None, vec![]));
        };
        let terminators = sqlx::query_as(
            "SELECT event_type, payload->>'request_event_id' FROM events \
             WHERE thread_id = $1 \
               AND event_type IN ('ResponseGenerated', 'ResponseCanceled', \
                                  'ResponseAborted', 'ResponseFailed') \
             ORDER BY sequence",
        )
        .bind(tid)
        .fetch_all(&pool)
        .await
        .map_err(|e| format!("query terminators: {e}"))?;
        Ok((Some(tid), terminators))
    }
    .await;
    pool.close().await;
    result
}

pub(crate) async fn run_trigger_with_a_free_slot(
    client: &reqwest::Client,
    id: &str,
) -> Result<serde_json::Value, String> {
    let (run_client, id) = (client.clone(), id.to_string());
    crate::support::with_pool_room(
        client,
        async move { try_run_trigger(&run_client, &id).await },
    )
    .await
}

async fn wait_for_last_run(
    client: &reqwest::Client,
    name: &str,
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(t) = find_trigger(client, name).await {
            if let Some(last_run) = t["last_run"].as_str() {
                return Some(last_run.to_string());
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    None
}

#[tokio::test]
async fn run_fires_a_cron_trigger_and_records_last_run() {
    let client = user_client().await;
    let slug = unique_marker("run-probe");
    let name = format!("Run probe {}", slug);
    let script = format!("triggers/{}/scripts/run.py", slug);
    {
        let _tree = crate::support::workspace_tree_lock().read().await;
        write_data_fixture(&client, &script, PROBE_SCRIPT)
            .await
            .expect("write the probe script");
    }
    let id = create_trigger(
        &client,
        &name,
        &slug,
        json!({ "type": "script", "path": script }),
        json!({ "cron_expressions": [NEVER_SOON_CRON] }),
    )
    .await;

    let before = find_trigger(&client, &name).await.expect("trigger listed");
    assert!(
        before["last_run"].is_null(),
        "fresh trigger must not have run yet: {before}"
    );

    let run = run_trigger_with_a_free_slot(&client, &id).await;

    let last_run = wait_for_last_run(&client, &name, Duration::from_secs(45)).await;
    delete_trigger(&client, &id).await;
    remove_data_fixtures(&client, &format!("triggers/{slug}"), &[script]).await;

    let resp = run.expect("off-schedule run request");
    assert_eq!(resp["success"], true, "run refused: {resp}");
    assert_eq!(
        resp["status"], "started",
        "an idle trigger's run must start, not queue or coalesce: {resp}"
    );
    assert!(
        last_run.is_some(),
        "the run never recorded last_run, so nothing actually fired"
    );
}

#[tokio::test]
async fn a_fired_trigger_turn_emits_exactly_one_terminator() {
    let client = user_client().await;
    let slug = unique_marker("run-terminator");
    let name = format!("Run terminator {}", slug);
    let id = create_trigger(
        &client,
        &name,
        &slug,
        json!({
            "type": "intent",
            "intent": "Reply with one short sentence confirming you ran. Call no tools.",
        }),
        json!({ "cron_expressions": [NEVER_SOON_CRON] }),
    )
    .await;

    let run = run_trigger_with_a_free_slot(&client, &id).await;

    let last_run = wait_for_last_run(&client, &name, Duration::from_secs(90)).await;
    let probe = read_turn_terminators(&id).await;

    delete_trigger(&client, &id).await;

    let run = run.expect("off-schedule run request");
    let (thread_id, terminators) = probe.expect("read the fired turn's terminators");

    assert_eq!(run["success"], true, "run refused: {run}");
    assert!(
        last_run.is_some(),
        "the run never recorded last_run, so nothing actually fired"
    );
    assert!(
        thread_id.is_some(),
        "the fire recorded no TriggerStarted, so there is no turn to check"
    );
    assert_eq!(
        terminators.len(),
        1,
        "a trigger fire is one turn and must leave exactly one terminator; found: {terminators:?}"
    );
    assert!(
        terminators[0].1.is_some(),
        "the terminator must be anchored on the turn it ends via request_event_id, \
         or it folds into whatever exchange happens to be last; found: {terminators:?}"
    );
}

#[tokio::test]
async fn run_refuses_a_paused_trigger_instead_of_silently_dropping_it() {
    let client = user_client().await;
    let slug = unique_marker("run-paused");
    let name = format!("Run paused {}", slug);
    let id = create_trigger(
        &client,
        &name,
        &slug,
        unreachable_intent(),
        json!({ "cron_expressions": [NEVER_SOON_CRON] }),
    )
    .await;

    let paused: serde_json::Value = client
        .put(format!("{}/api/v1/triggers?id={}", base_url(), id))
        .json(&json!({ "paused": true }))
        .send()
        .await
        .expect("PUT /triggers failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert_eq!(paused["success"], true, "pause failed: {paused}");

    let resp = run_trigger(&client, &id).await;
    let after = find_trigger(&client, &name).await.expect("trigger listed");
    delete_trigger(&client, &id).await;

    assert_eq!(
        resp["success"], false,
        "a paused trigger's run must be refused, not reported as started: {resp}"
    );
    let message = resp["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("paused"),
        "the refusal must say the trigger is paused: {resp}"
    );
    assert!(
        after["last_run"].is_null(),
        "nothing may have run for a paused trigger: {after}"
    );
}

#[tokio::test]
async fn run_refuses_an_event_only_trigger_and_points_at_emitting_the_event() {
    let client = user_client().await;
    let slug = unique_marker("run-eventonly");
    let name = format!("Run event-only {}", slug);
    let event_type = format!("E2eRunProbe{}", slug.replace('-', ""));
    let id = create_trigger(
        &client,
        &name,
        &slug,
        unreachable_intent(),
        json!({ "on": [{ "event_type": event_type }] }),
    )
    .await;

    let resp = run_trigger(&client, &id).await;
    delete_trigger(&client, &id).await;

    assert_eq!(
        resp["success"], false,
        "an event-only trigger has no scheduled fire to reproduce: {resp}"
    );
    let message = resp["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&event_type),
        "the refusal must name the event the trigger subscribes to: {resp}"
    );
    assert!(
        message.contains("Emit"),
        "the refusal must point at the route that does work: {resp}"
    );
}

#[tokio::test]
async fn run_refuses_an_unknown_trigger_id() {
    let client = user_client().await;
    let resp = run_trigger(&client, "00000000-0000-0000-0000-000000000000").await;
    assert_eq!(resp["success"], false, "unknown id must be refused: {resp}");
    assert!(
        resp["message"]
            .as_str()
            .unwrap_or_default()
            .contains("No trigger found"),
        "the refusal must say the id is unknown: {resp}"
    );
}
