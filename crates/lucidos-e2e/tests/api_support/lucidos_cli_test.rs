//! E2E test for the `lucidos` CLI against a running Lucidos workspace.
//!
//! Verifies the headline use case from the spec: an `events emit` from a
//! Claude-Code-shaped subprocess (worktree subdir, `LUCIDOS_WORKSPACE` set) is
//! observable via `events query` immediately afterwards.

use crate::support::{unique_marker, workspace_path};
use std::process::Command;

fn lucidos_bin() -> std::path::PathBuf {
    // Integration tests live at target/<profile>/deps/<test-binary>. The
    // sibling `lucidos` CLI binary sits at target/<profile>/lucidos. We can
    // find it by walking up two directories from the current test executable.
    let test_exe = std::env::current_exe().expect("current_exe");
    let target_profile_dir = test_exe
        .parent()
        .and_then(|p| p.parent())
        .expect("test exe must live under target/<profile>/deps/");
    let bin = target_profile_dir.join("lucidos");
    assert!(
        bin.exists(),
        "lucidos CLI binary not found at {}. Run `cargo build -p lucidos-cli` first.",
        bin.display()
    );
    bin
}

#[test]
fn emit_then_query_round_trip_against_running_workspace() {
    let bin = lucidos_bin();

    // Use the e2e-test workspace exactly the way an engine-spawned CC would:
    // PWD outside the workspace, LUCIDOS_WORKSPACE pointing at it.
    let ws = workspace_path();
    let event_type = "LucidosCliE2eEmitted";
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
        .env("LUCIDOS_WORKSPACE", &ws)
        .output()
        .expect("lucidos events emit should run");
    assert!(
        emit.status.success(),
        "emit failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&emit.stdout),
        String::from_utf8_lossy(&emit.stderr)
    );

    // Query and find our marker
    let query = Command::new(&bin)
        .args(["events", "query", "--type", event_type, "--limit", "10"])
        .env("LUCIDOS_WORKSPACE", &ws)
        .output()
        .expect("lucidos events query should run");
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
fn emit_rejects_missing_summary() {
    let bin = lucidos_bin();
    let ws = workspace_path();
    let out = Command::new(&bin)
        .args([
            "events",
            "emit",
            "LucidosCliE2eRejected",
            "--payload",
            "{\"foo\": 1}",
        ])
        .env("LUCIDOS_WORKSPACE", &ws)
        .output()
        .expect("lucidos should run");
    assert!(!out.status.success(), "emit without summary must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("summary"),
        "stderr should mention summary: {stderr}"
    );
}

/// Regression for the bug where `harden.md` Phase 0 read a filesystem marker
/// no current code wrote (post-DB-migration), causing CC to falsely report
/// `ALREADY_HARDENED` after engine restart interruption. The skill now reads
/// state via `lucidos hardened query`; this round-trip proves the CLI surface
/// reports `MISSING → FRESH → STALE` correctly against the real engine.
#[test]
fn hardened_query_round_trip() {
    let bin = lucidos_bin();
    let ws = workspace_path();

    // Throwaway git repo so we don't pollute the e2e workspace's hardened_branches
    // rows for actual branches. Unique branch name keeps state isolated across runs.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    let branch = format!("cli-test-{}", unique_marker("hardened"));

    run_git(repo, &["init", "-q", "-b", "main"]);
    run_git(repo, &["config", "user.email", "test@example.com"]);
    run_git(repo, &["config", "user.name", "test"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    run_git(repo, &["add", "."]);
    run_git(repo, &["commit", "-q", "-m", "first"]);
    run_git(repo, &["checkout", "-q", "-b", &branch]);

    // 1) MISSING — nothing recorded yet for this (repo, branch).
    let out = Command::new(&bin)
        .args(["hardened", "query"])
        .current_dir(repo)
        .env("LUCIDOS_WORKSPACE", &ws)
        .output()
        .expect("lucidos hardened query should run");
    assert!(
        out.status.success(),
        "query failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "MISSING",
        "fresh branch must report MISSING"
    );

    // 2) Mark — should record current HEAD.
    let mark = Command::new(&bin)
        .args(["hardened", "mark"])
        .current_dir(repo)
        .env("LUCIDOS_WORKSPACE", &ws)
        .output()
        .expect("lucidos hardened mark should run");
    assert!(
        mark.status.success(),
        "mark failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&mark.stdout),
        String::from_utf8_lossy(&mark.stderr)
    );

    // 3) FRESH — HEAD still matches the just-recorded SHA.
    let out = Command::new(&bin)
        .args(["hardened", "query"])
        .current_dir(repo)
        .env("LUCIDOS_WORKSPACE", &ws)
        .output()
        .expect("lucidos hardened query should run");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "FRESH",
        "after mark, query must report FRESH"
    );

    // 4) STALE — advance HEAD; recorded SHA no longer matches.
    std::fs::write(repo.join("b.txt"), "b").unwrap();
    run_git(repo, &["add", "."]);
    run_git(repo, &["commit", "-q", "-m", "second"]);

    let out = Command::new(&bin)
        .args(["hardened", "query"])
        .current_dir(repo)
        .env("LUCIDOS_WORKSPACE", &ws)
        .output()
        .expect("lucidos hardened query should run");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "STALE",
        "after new commit, query must report STALE"
    );
}

fn run_git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {} failed: {}", args.join(" "), e));
    assert!(
        out.status.success(),
        "git {} in {} failed: {}",
        args.join(" "),
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}
