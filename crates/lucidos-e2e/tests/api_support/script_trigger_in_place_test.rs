//! E2E coverage for the in-place *script* execution path: a script trigger's
//! `.py` file must be executed from its REAL on-disk location, so a
//! `__file__`-relative sibling path resolves inside the trigger's own directory.
//!
//! The unit tests in `runtime/python_tests.rs` pin `execute_file_with_env`
//! itself; this one pins the wiring end to end — trigger create → domain event
//! → scheduler fan-out → `execute_script` → interpreter — because the bug was
//! precisely a wrong call at that seam (`read_to_string` + `execute_with_env`,
//! which runs a copy under `.lucidos/exhaust/<uuid>/script.py`). With the bug,
//! `dirname(__file__)/../state` pointed at a phantom `.lucidos/exhaust/state/`:
//! the 2026-07-29 `notary-verdict-watch` trigger read a default instead of the
//! user's recorded DMG approval, withheld a release publish, and said so.

use crate::support::{base_url, http_client, unique_marker, workspace_path};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Reads its sibling `../state/marker.json` the ordinary way and writes the
/// verdict back to the same directory. Under the bug both paths land in
/// `.lucidos/exhaust/state/` and the real `result.json` never appears.
const PROBE_SCRIPT: &str = r#"#!/usr/bin/env python3
import json, os

_HERE = os.path.dirname(os.path.abspath(__file__))
_STATE = os.path.join(_HERE, "..", "state")

try:
    with open(os.path.join(_STATE, "marker.json")) as f:
        approved = json.load(f)["approved_version"]
except FileNotFoundError:
    approved = "MISSING"

os.makedirs(_STATE, exist_ok=True)
with open(os.path.join(_STATE, "result.json"), "w") as f:
    json.dump({"approved_version": approved, "file": os.path.abspath(__file__)}, f)
print("ok")
"#;

/// Async only so it can take the shared-tree read guard: these files appearing
/// in the working tree must not land mid-snapshot for the command-checkpoint
/// test (see `workspace_tree_lock`).
async fn write_probe_trigger(slug: &str) -> PathBuf {
    let _tree = crate::support::workspace_tree_lock().read().await;
    let trigger_dir = workspace_path().join("data/triggers").join(slug);
    std::fs::create_dir_all(trigger_dir.join("scripts")).expect("create scripts dir");
    std::fs::create_dir_all(trigger_dir.join("state")).expect("create state dir");
    std::fs::write(trigger_dir.join("scripts/run.py"), PROBE_SCRIPT).expect("write script");
    std::fs::write(
        trigger_dir.join("state/marker.json"),
        // A sentinel, deliberately NOT a real release version: a literal equal to
        // RELEASE would trip version_sources_test.sh's unmanaged-literal scan.
        r#"{"approved_version": "0.0.0-fixture"}"#,
    )
    .expect("write marker");
    trigger_dir
}

async fn find_trigger_id(client: &reqwest::Client, name: &str) -> Option<String> {
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
        .and_then(|t| t["id"].as_str().map(str::to_string))
}

/// Remove the probe trigger's directory AND commit the removal. The engine
/// auto-commits dirty `data/` files after every script run, so the probe files
/// are tracked by the time we get here — deleting them without committing
/// would leave the e2e workspace's working tree dirty for every later test.
/// Best-effort: the engine may hold `index.lock` for another run's auto-commit,
/// and a failed cleanup must not fail the assertion this test exists for.
async fn remove_and_commit(trigger_dir: &Path, slug: &str) {
    // Removing them is a working-tree change too; same guard as the write.
    let _tree = crate::support::workspace_tree_lock().read().await;
    let _ = std::fs::remove_dir_all(trigger_dir);
    let pathspec = format!("data/triggers/{}", slug);
    // Pathspec form: commits the working-tree state of exactly these paths,
    // so a concurrent test's staged changes are never swept in.
    let _ = std::process::Command::new("git")
        .current_dir(workspace_path())
        .args([
            "commit",
            "-q",
            "-m",
            "e2e: remove in-place probe trigger",
            "--",
            &pathspec,
        ])
        .output();
}

async fn wait_for_file(path: &Path, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(content) = std::fs::read_to_string(path) {
            return Some(content);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    None
}

#[tokio::test]
async fn script_trigger_runs_in_place_and_reaches_its_sibling_state_dir() {
    let client = http_client();
    let ws = workspace_path();
    let slug = unique_marker("inplace-probe");
    let name = format!("In-place probe {}", slug);
    let event_type = format!("E2eInPlaceProbe{}", slug.replace('-', ""));

    let trigger_dir = write_probe_trigger(&slug).await;

    let created: serde_json::Value = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&json!({
            "name": name,
            "slug": slug,
            "on": [{ "event_type": event_type }],
            "run": {
                "type": "script",
                "path": format!("triggers/{}/scripts/run.py", slug),
            },
        }))
        .send()
        .await
        .expect("POST /triggers failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert_eq!(created["success"], true, "create failed: {created}");

    let resp: serde_json::Value = client
        .post(format!("{}/api/v1/events/emit", base_url()))
        .json(&json!({ "event_type": event_type, "payload": { "summary": "probe" } }))
        .send()
        .await
        .expect("POST /events/emit failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert_eq!(resp["success"], true, "emit failed: {resp}");

    let result = wait_for_file(
        &trigger_dir.join("state/result.json"),
        Duration::from_secs(45),
    )
    .await;

    // Tear down before asserting so a failure doesn't leave a live trigger
    // pointed at files the cleanup removes.
    let phantom = ws.join(".lucidos/exhaust/state");
    let phantom_existed = phantom.exists();
    if let Some(id) = find_trigger_id(&client, &name).await {
        let _ = client
            .delete(format!("{}/api/v1/triggers?id={}", base_url(), id))
            .send()
            .await;
    }
    remove_and_commit(&trigger_dir, &slug).await;
    let _ = std::fs::remove_dir_all(&phantom);

    assert!(
        !phantom_existed,
        "the script wrote into .lucidos/exhaust/state — it ran from a copy, not its real path"
    );
    let result = result.expect(
        "the script never wrote state/result.json next to itself — \
         it ran from a copy under .lucidos/exhaust and its __file__-relative paths went elsewhere",
    );
    let parsed: serde_json::Value = serde_json::from_str(&result).expect("result.json is JSON");

    assert_eq!(
        parsed["approved_version"], "0.0.0-fixture",
        "script read a default instead of the real sibling state file: {parsed}"
    );
    let reported_file = parsed["file"].as_str().expect("file field");
    assert!(
        !reported_file.contains("exhaust"),
        "__file__ pointed into the exhaust dir: {reported_file}"
    );
    assert!(
        reported_file.ends_with(&format!("data/triggers/{}/scripts/run.py", slug)),
        "__file__ must be the script's real on-disk path, got: {reported_file}"
    );
}
