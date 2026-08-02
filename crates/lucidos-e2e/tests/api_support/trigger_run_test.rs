//! E2E coverage for `POST /api/v1/triggers/run`, the **off-schedule run**.
//!
//! Unit tests in `engine_impl::trigger_runs` pin the pure precondition checker;
//! this pins the wiring end to end over real HTTP, because the operation exists
//! precisely to replace a workaround that *looked* like it worked. The three
//! things worth proving at this seam:
//!
//! 1. A cron trigger actually fires and records `last_run`, so the run is real
//!    rather than an admission that goes nowhere.
//! 2. A paused trigger is refused up front. Submitting blind would be dropped
//!    by the queue executor with only a log line, and the caller would be told
//!    a run started, which is the exact class of lie this replaces (an agent
//!    once "started" a nightly job by resuming a paused trigger).
//! 3. An event-only trigger is refused and pointed at emitting its event. A
//!    payload-less fire is a shape it has never had.

use crate::support::{base_url, http_client, unique_marker, workspace_path};
use serde_json::json;
use std::time::{Duration, Instant};

/// A script trigger, so a fire needs no LLM provider to complete. The script
/// writes nothing and exits 0; `last_run` is what we assert on.
const PROBE_SCRIPT: &str = r#"#!/usr/bin/env python3
print("ok")
"#;

/// Cron pinned to a moment that never arrives inside a test run (03:00 on the
/// 1st of January). The point is that the trigger HAS a schedule, not that the
/// schedule fires: an off-schedule run must be the only thing that runs it, so
/// a passing assertion can't be a scheduled fire in disguise.
const NEVER_SOON_CRON: &str = "0 0 3 1 1 *";

/// Create a trigger and return its id.
///
/// **`run` is an intent unless the test actually fires the trigger.** A script
/// trigger needs a `.py` under `data/triggers/<slug>/`, and `data/` is
/// git-tracked, so the file leaves the e2e workspace's working tree dirty from
/// the moment it is written until the engine's post-run auto-commit picks it
/// up. A trigger that never fires never gets that auto-commit, so the dirt
/// lasts the whole test, and the concurrently-running apply tests fail with
/// "Cannot merge: the repository has uncommitted changes". The three refusal
/// tests below never reach execution, so they carry an intent they will never
/// run and touch no files at all.
async fn create_trigger(
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

/// A never-executed `run` for the refusal tests. Naming it makes the "this
/// trigger is not meant to fire" contract explicit at each call site.
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
    client
        .post(format!("{}/api/v1/triggers/run?id={}", base_url(), id))
        .send()
        .await
        .expect("POST /triggers/run failed")
        .json()
        .await
        .expect("Invalid JSON")
}

/// Delete a trigger that owns no files on disk. Best-effort: a failed cleanup
/// must not fail the assertion the test exists for.
async fn delete_trigger(client: &reqwest::Client, id: &str) {
    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), id))
        .send()
        .await;
}

/// Delete a script trigger AND remove its directory, committing the removal so
/// the e2e workspace's working tree is left clean (the engine auto-commits
/// dirty `data/` files after a script run, so the probe file is tracked by
/// now). A dirty tree fails every concurrent apply test with "Cannot merge:
/// the repository has uncommitted changes", so this is load-bearing, not
/// tidiness. Best-effort throughout.
async fn cleanup_script_trigger(client: &reqwest::Client, id: &str, slug: &str) {
    delete_trigger(client, id).await;
    let _ = std::fs::remove_dir_all(workspace_path().join("data/triggers").join(slug));
    // Pathspec form: commits the working-tree state of exactly these paths, so
    // a concurrent test's staged changes are never swept in.
    let _ = std::process::Command::new("git")
        .current_dir(workspace_path())
        .args([
            "commit",
            "-q",
            "-m",
            "e2e: remove off-schedule-run probe trigger",
            "--",
            &format!("data/triggers/{}", slug),
        ])
        .output();
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
    let client = http_client();
    let slug = unique_marker("run-probe");
    let name = format!("Run probe {}", slug);
    // The one test that actually fires, so the one that needs a real script on
    // disk (a script run needs no LLM provider). The engine auto-commits it
    // after the run; `cleanup_script_trigger` commits its removal.
    let trigger_dir = workspace_path().join("data/triggers").join(&slug);
    std::fs::create_dir_all(trigger_dir.join("scripts")).expect("create scripts dir");
    std::fs::write(trigger_dir.join("scripts/run.py"), PROBE_SCRIPT).expect("write script");
    let id = create_trigger(
        &client,
        &name,
        &slug,
        json!({ "type": "script", "path": format!("triggers/{}/scripts/run.py", slug) }),
        json!({ "cron_expressions": [NEVER_SOON_CRON] }),
    )
    .await;

    let before = find_trigger(&client, &name).await.expect("trigger listed");
    assert!(
        before["last_run"].is_null(),
        "fresh trigger must not have run yet: {before}"
    );

    let resp = run_trigger(&client, &id).await;
    let last_run = wait_for_last_run(&client, &name, Duration::from_secs(45)).await;
    cleanup_script_trigger(&client, &id, &slug).await;

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
async fn run_refuses_a_paused_trigger_instead_of_silently_dropping_it() {
    let client = http_client();
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
    let client = http_client();
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
    let client = http_client();
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
