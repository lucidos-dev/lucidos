//! E2E test for the `cognos` CLI against a running CognOS workspace.
//!
//! Verifies the headline use case from the spec: an `events emit` from a
//! Claude-Code-shaped subprocess (worktree subdir, `COGNOS_WORKSPACE` set) is
//! observable via `events query` immediately afterwards.

use crate::support::{unique_marker, workspace_path};
use std::process::Command;

fn cognos_bin() -> std::path::PathBuf {
    // CARGO_BIN_EXE_* is only set for the current crate's own binaries, so
    // locate `cognos` (a sibling crate's binary) via the engine binary path.
    let engine_bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_cognos-engine"));
    let bin = engine_bin
        .parent()
        .expect("engine bin must have a parent")
        .join("cognos");
    assert!(
        bin.exists(),
        "cognos CLI binary not found at {}. Run `cargo build -p cognos-cli` first.",
        bin.display()
    );
    bin
}

#[test]
#[ignore]
fn emit_then_query_round_trip_against_running_workspace() {
    let bin = cognos_bin();

    // Use the e2e-test workspace exactly the way an engine-spawned CC would:
    // PWD outside the workspace, COGNOS_WORKSPACE pointing at it.
    let ws = workspace_path();
    let event_type = "CognosCliE2eEmitted";
    let summary = unique_marker("cli-e2e");

    let payload = serde_json::json!({ "marker": summary }).to_string();

    // Emit
    let emit = Command::new(&bin)
        .args([
            "events",
            "emit",
            event_type,
            "--summary",
            &summary,
            "--payload",
        ])
        .arg(&payload)
        .env("COGNOS_WORKSPACE", &ws)
        .output()
        .expect("cognos events emit should run");
    assert!(
        emit.status.success(),
        "emit failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&emit.stdout),
        String::from_utf8_lossy(&emit.stderr)
    );

    // Query and find our marker
    let query = Command::new(&bin)
        .args(["events", "query", "--type", event_type, "--limit", "10"])
        .env("COGNOS_WORKSPACE", &ws)
        .output()
        .expect("cognos events query should run");
    assert!(
        query.status.success(),
        "query failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&query.stdout),
        String::from_utf8_lossy(&query.stderr)
    );

    let body: serde_json::Value =
        serde_json::from_slice(&query.stdout).expect("query stdout must be JSON");
    let arr = body.as_array().expect("query response must be an array");

    let found = arr.iter().any(|ev| {
        ev["payload"]["summary"].as_str() == Some(summary.as_str())
            && ev["payload"]["marker"].as_str() == Some(summary.as_str())
    });
    assert!(
        found,
        "Did not find emitted event with marker {summary} in query response: {}",
        serde_json::to_string_pretty(&body).unwrap()
    );
}

#[test]
#[ignore]
fn emit_rejects_missing_summary() {
    let bin = cognos_bin();
    let ws = workspace_path();
    let out = Command::new(&bin)
        .args([
            "events",
            "emit",
            "CognosCliE2eRejected",
            "--payload",
            "{\"foo\": 1}",
        ])
        .env("COGNOS_WORKSPACE", &ws)
        .output()
        .expect("cognos should run");
    assert!(!out.status.success(), "emit without summary must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("summary"),
        "stderr should mention summary: {stderr}"
    );
}
