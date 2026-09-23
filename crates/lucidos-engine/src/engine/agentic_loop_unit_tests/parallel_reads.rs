//! Which calls of one batch may run together, and how the run behaves.
//! ADR 0246 holds the reasoning.

use super::{
    is_parallel_safe, parallel_run_end, run_concurrently, run_tool_with_cancel,
    MAX_PARALLEL_TOOL_CALLS, PARALLEL_SAFE_TOOLS,
};
use crate::engine::command_guard::{static_classify, RiskLane, StaticVerdict};
use crate::engine::tools::ToolOutcome;
use crate::llm::provider::ToolCall;
use crate::llm::tool_names as tn;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: format!("toolu_{name}"),
        name: name.to_string(),
        arguments,
        thought_signature: None,
    }
}

fn read(path: &str) -> ToolCall {
    call(tn::READ_FILE, json!({ "path": path }))
}

fn write(path: &str) -> ToolCall {
    call(tn::WRITE_FILE, json!({ "path": path, "content": "x" }))
}

/// Skipping the guard for a run is only sound if the guard would have
/// waved every one of these through anyway. The args carry a command the
/// guard hard-blocks, so a tool that reads `command` would fail here.
#[test]
fn no_parallel_safe_tool_has_a_command_guard_lane() {
    let destructive = json!({ "command": "rm -rf /", "code": "import shutil; shutil.rmtree('/')" });
    for name in PARALLEL_SAFE_TOOLS {
        assert_eq!(
            static_classify(name, &destructive),
            StaticVerdict::Settled(RiskLane::Safe),
            "{name} is classified by the command guard, so it cannot skip it in a parallel run"
        );
    }
}

/// A run calls `execute_tool` directly, so a tool the loop or
/// `handle_special_tool` intercepts would lose its special handling.
#[test]
fn no_parallel_safe_tool_is_intercepted_before_execute_tool() {
    let intercepted = [
        tn::ASK_USER_QUESTION,
        tn::CAPTURE_APP,
        tn::REFRESH_APP,
        tn::RUN_CODING_AGENT,
        tn::RUN_CLAUDE_LEGACY,
        tn::RUN_THREAD,
        tn::TODO_WRITE,
        tn::AWAIT_EVENT,
        tn::LIST_EVENT_WAITS,
        tn::CANCEL_EVENT_WAIT,
    ];
    for name in PARALLEL_SAFE_TOOLS {
        assert!(
            !intercepted.contains(name),
            "{name} is intercepted before execute_tool"
        );
        assert!(
            crate::mcp::McpManager::parse_mcp_tool_name(name).is_none(),
            "{name} routes to an MCP server"
        );
    }
}

#[test]
fn writes_commands_and_spawns_are_not_parallel_safe() {
    for name in [
        tn::WRITE_FILE,
        tn::EDIT_FILE,
        tn::DELETE_FILE,
        tn::RUN_BASH,
        tn::RUN_PYTHON,
        tn::BROWSER_OPEN,
        tn::HTTP_REQUEST,
        tn::ASK_USER_QUESTION,
        tn::RUN_THREAD,
        tn::EMIT_EVENT,
    ] {
        assert!(!is_parallel_safe(name, &json!({})), "{name} must run alone");
    }
}

/// The model sees the grouped `events` tool, so the allowlist has to follow
/// its `action` the way `execute_tool` does.
#[test]
fn a_grouped_tool_is_parallel_safe_only_for_a_read_action() {
    assert!(is_parallel_safe(tn::EVENTS, &json!({ "action": "query" })));
    assert!(!is_parallel_safe(tn::EVENTS, &json!({ "action": "emit" })));
    assert!(!is_parallel_safe(tn::EVENTS, &json!({})));
}

/// A flat alias runs as itself whatever `action` it carries. Reading the
/// action here would pass a write as the read it names, and run it in a run.
#[test]
fn a_flat_alias_is_judged_by_its_own_name_not_its_action() {
    assert!(!is_parallel_safe(
        tn::EMIT_EVENT,
        &json!({ "action": "query" })
    ));
    assert!(is_parallel_safe(
        tn::QUERY_EVENTS,
        &json!({ "action": "emit" })
    ));
}

#[test]
fn a_single_read_is_a_run_of_one() {
    let calls = [read("a")];
    assert_eq!(parallel_run_end(&calls, 0), 1);
}

#[test]
fn a_run_stops_at_the_first_call_that_must_run_alone() {
    let calls = [read("a"), read("b"), write("c"), read("c"), read("d")];
    assert_eq!(
        parallel_run_end(&calls, 0),
        2,
        "the write ends the first run"
    );
    assert_eq!(parallel_run_end(&calls, 2), 2, "the write is no run at all");
    assert_eq!(
        parallel_run_end(&calls, 3),
        5,
        "the reads after it form their own run"
    );
}

#[test]
fn a_read_after_a_write_never_joins_the_read_before_it() {
    let calls = [read("a"), write("a"), read("a")];
    assert_eq!(parallel_run_end(&calls, 0), 1);
    assert_eq!(parallel_run_end(&calls, 2), 3);
}

async fn job(delay_ms: u64, text: &'static str) -> ToolOutcome {
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    Ok(text.to_string())
}

/// The slowest call comes first, so ordering by finish time would reorder
/// the results, and running in sequence would take the sum of the delays.
#[tokio::test(start_paused = true)]
async fn outcomes_come_back_in_call_order_and_the_calls_overlap() {
    let started = tokio::time::Instant::now();
    let outcomes = run_concurrently(vec![job(30, "a"), job(10, "b"), job(20, "c")]).await;

    assert_eq!(
        outcomes,
        vec![
            Ok("a".to_string()),
            Ok("b".to_string()),
            Ok("c".to_string())
        ]
    );
    assert_eq!(started.elapsed(), Duration::from_millis(30));
}

/// A call's own emits run inside its future, so they land when it finishes.
/// That is what makes a fast call's step resolve before a slow one's.
#[tokio::test(start_paused = true)]
async fn each_call_reports_when_it_finishes_not_in_call_order() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let reporting = |delay_ms: u64, label: &'static str| {
        let log = log.clone();
        async move {
            let outcome = job(delay_ms, label).await;
            log.lock().unwrap().push(label);
            outcome
        }
    };

    let outcomes = run_concurrently(vec![
        reporting(30, "a"),
        reporting(10, "b"),
        reporting(20, "c"),
    ])
    .await;

    assert_eq!(*log.lock().unwrap(), vec!["b", "c", "a"]);
    assert_eq!(outcomes.len(), 3);
}

async fn counted_job(in_flight: Arc<AtomicUsize>, peak: Arc<AtomicUsize>) -> ToolOutcome {
    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    peak.fetch_max(now, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(10)).await;
    in_flight.fetch_sub(1, Ordering::SeqCst);
    Ok(String::new())
}

#[tokio::test(start_paused = true)]
async fn no_more_than_the_cap_run_at_once() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let calls = (0..10)
        .map(|_| counted_job(in_flight.clone(), peak.clone()))
        .collect();

    let outcomes = run_concurrently(calls).await;

    assert_eq!(outcomes.len(), 10);
    assert_eq!(peak.load(Ordering::SeqCst), MAX_PARALLEL_TOOL_CALLS);
}

/// One slow call holds one slot, never the others. Eight fast calls share the
/// other three slots, so the run ends with the slow call at 100 ms. An
/// ordered buffer would hold the finished slots and end at 120 ms.
#[tokio::test(start_paused = true)]
async fn a_slow_call_does_not_hold_up_the_calls_behind_it() {
    let started = tokio::time::Instant::now();
    let mut calls = vec![job(100, "slow")];
    calls.extend((0..8).map(|_| job(10, "fast")));

    let outcomes = run_concurrently(calls).await;

    assert_eq!(outcomes.len(), 9);
    assert_eq!(started.elapsed(), Duration::from_millis(100));
}

/// A Stop settles every call, and every call still reports both ends. The
/// cancel races only the work, so no step is left without its result.
#[tokio::test(start_paused = true)]
async fn a_stop_settles_every_call_and_each_still_reports_both_ends() {
    let token = CancellationToken::new();
    let canceler = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        canceler.cancel();
    });
    let log = Arc::new(Mutex::new(Vec::new()));
    let calls = (0..6)
        .map(|i| {
            let (log, token) = (log.clone(), token.clone());
            async move {
                log.lock().unwrap().push(format!("start {i}"));
                let outcome = run_tool_with_cancel(job(3_600_000, "never"), &token).await;
                log.lock().unwrap().push(format!("end {i}"));
                outcome
            }
        })
        .collect();
    let started = tokio::time::Instant::now();

    let outcomes = run_concurrently(calls).await;

    for outcome in &outcomes {
        assert_eq!(outcome, &Err("Error: canceled by user".to_string()));
    }
    let log = log.lock().unwrap();
    for i in 0..6 {
        assert!(log.contains(&format!("start {i}")) && log.contains(&format!("end {i}")));
    }
    assert_eq!(started.elapsed(), Duration::from_millis(5));
}
