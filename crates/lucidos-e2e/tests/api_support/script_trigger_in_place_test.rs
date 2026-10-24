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

use crate::support::{
    base_url, remove_data_fixtures, unique_marker, user_client, workspace_path, write_data_fixture,
};
use serde_json::json;
use std::path::Path;
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

/// The probe's files, `data/`-relative: the two it is given, then the one its
/// run writes.
fn probe_files(slug: &str) -> [String; 3] {
    ["scripts/run.py", "state/marker.json", "state/result.json"]
        .map(|f| format!("triggers/{slug}/{f}"))
}

/// Write the script and its sibling state through the data API, so both land
/// committed. Holds the shared-tree read guard: these files appearing must not
/// land mid-snapshot for the command-checkpoint test (see
/// `workspace_tree_lock`).
async fn write_probe_trigger(client: &reqwest::Client, slug: &str) {
    let [script, marker, _] = probe_files(slug);
    let _tree = crate::support::workspace_tree_lock().read().await;
    write_data_fixture(client, &script, PROBE_SCRIPT)
        .await
        .expect("write the probe script");
    write_data_fixture(
        client,
        &marker,
        // A sentinel, deliberately NOT a real release version: a literal equal to
        // RELEASE would trip version_sources_test.sh's unmanaged-literal scan.
        r#"{"approved_version": "0.0.0-fixture"}"#,
    )
    .await
    .expect("write the marker");
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
    let client = user_client().await;
    let ws = workspace_path();
    let slug = unique_marker("inplace-probe");
    let name = format!("In-place probe {}", slug);
    let event_type = format!("E2eInPlaceProbe{}", slug.replace('-', ""));

    let files = probe_files(&slug);
    write_probe_trigger(&client, &slug).await;

    let created: serde_json::Value = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&json!({
            "name": name,
            "slug": slug,
            "on": [{ "event_type": event_type }],
            "run": {
                "type": "script",
                "path": files[0],
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

    let result = wait_for_file(&ws.join("data").join(&files[2]), Duration::from_secs(45)).await;

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
    remove_data_fixtures(&client, &format!("triggers/{slug}"), &files).await;
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
