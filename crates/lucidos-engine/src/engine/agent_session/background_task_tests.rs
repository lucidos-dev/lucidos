//! The start decision for a coding agent's background task.
//!
//! One completion must reach the thread exactly once. A task a wait covers is
//! delivered by the wait, so its result must not also come back inline. A task
//! that finished before any wait existed has nobody else to report it.

use super::*;

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Covered wins even when the task has already finished: the wait holds the
/// completion, and an inline result as well would be the second delivery.
#[test]
fn a_covered_task_is_watched_whether_or_not_it_is_still_running() {
    let covered = ids(&["other", "mine"]);
    assert_eq!(settle_start("mine", &covered, true), Settled::Watched);
    assert_eq!(settle_start("mine", &covered, false), Settled::Watched);
}

/// Running with no wait is the cap refusal. The agent must be told, because
/// ending its turn now would leave nothing to wake it.
#[test]
fn a_running_task_no_wait_covers_is_unwatched() {
    assert_eq!(
        settle_start("mine", &ids(&["other"]), true),
        Settled::Unwatched
    );
}

/// A quick task can finish before the arming reads the registry. Then no wait
/// exists, and the result has to come back in the response.
#[test]
fn a_task_that_finished_before_any_wait_comes_back_inline() {
    assert_eq!(settle_start("mine", &[], false), Settled::AlreadyFinished);
}

/// A coding agent's shell holds none of the workspace's secrets. A task it
/// starts through the engine must not either, or the route hands them over:
/// the chat tool's env carries every `CRED_*` and `OAUTH_*` value.
#[test]
fn a_coding_agents_background_task_never_gets_the_workspace_secrets() {
    use crate::test_support::source_scan::{read_production_source, src_root};
    let this = read_production_source(&src_root().join("engine/agent_session/background_task.rs"));
    assert!(this.contains("build_agent_task_env_vars("), "{this}");
    assert!(
        !this.contains("build_tool_env_vars") && !this.contains("build_script_env_vars"),
        "the agent route must not take the chat tool's env"
    );

    let scripts = read_production_source(&src_root().join("engine/engine_impl/scripts.rs"));
    let start = scripts
        .find("pub(crate) async fn build_agent_task_env_vars(")
        .expect("the agent env builder");
    let body = &scripts[start..];
    let body = &body[..body.find("\n    }\n").expect("end of the builder")];
    for secret_source in ["secret_env_vars", "CredentialStore", "OAuthStore"] {
        assert!(
            !body.contains(secret_source),
            "the agent env must not reach `{secret_source}`:\n{body}"
        );
    }
}

/// The engine owns a background task, so nothing else reaps it when its thread
/// is thrown away. Apply and Cancel keep the work, so they leave it running.
#[test]
fn discard_and_archive_abandon_background_tasks_and_nothing_else_does() {
    use crate::engine::types::StopReason;
    assert!(StopReason::Discard.abandons_background_tasks());
    assert!(StopReason::Archive.abandons_background_tasks());
    assert!(!StopReason::Apply.abandons_background_tasks());
    assert!(!StopReason::UserStop.abandons_background_tasks());
}

/// The rule above only helps if `stop_agent` acts on it on both branches: a
/// Discard on a thread whose agent process already went idle takes the
/// no-session branch, while its task keeps running. The kill follows the
/// refusal check, so a refused stop kills nothing.
#[test]
fn stop_agent_kills_the_threads_tasks_whether_or_not_a_session_is_live() {
    let src = crate::test_support::source_scan::read_production_source(
        &crate::test_support::source_scan::src_root().join("engine/claude_code/control.rs"),
    );
    let body = &src[src.find("pub async fn stop_agent(").expect("stop_agent")..];
    let kill = body
        .find("abandon_background_tasks(")
        .expect("stop_agent must kill the thread's background tasks");
    let refusal = body
        .find("stop_refusal(")
        .expect("stop_agent asks whether the stop is refused");
    let branch = body
        .find("if let Some(stop) = reserved")
        .expect("stop_agent branches on the session it reserved");
    assert!(refusal < kill, "a refused stop must kill nothing");
    assert!(kill < branch, "the kill must not depend on a live session");
    assert!(
        body[..kill].contains("abandons_background_tasks()"),
        "the kill is gated on the stop reason"
    );
}

/// An archive usually finds no live session, so `stop_agent` never runs for
/// it. The bus subscriber is what reaches those threads, on exactly the two
/// events that end a thread's work.
#[test]
fn the_reaper_fires_on_an_archived_or_discarded_thread_only() {
    use crate::engine::thread_events::ThreadEvent;
    assert!(ends_the_threads_work(&ThreadEvent::ThreadArchived));
    assert!(ends_the_threads_work(&ThreadEvent::ThreadDiscarded {
        actor: None
    }));
    assert!(!ends_the_threads_work(&ThreadEvent::ThreadSaved));

    let main = crate::test_support::source_scan::read_production_source(
        &crate::test_support::source_scan::src_root().join("main.rs"),
    );
    assert!(
        main.contains("start_background_task_reaper()"),
        "the engine must start the reaper at boot"
    );
}

/// Every user discard resets the worktree a task may still be running in.
/// The change card, the bulk discard and `/claude-code/discard` all reach one
/// of these two, and the thread-level one covers a thread with no change yet.
#[test]
fn every_user_discard_kills_the_threads_background_tasks() {
    let src = crate::test_support::source_scan::read_production_source(
        &crate::test_support::source_scan::src_root().join("engine/change_ops/discard.rs"),
    );
    for entry in [
        "pub async fn discard_pending_for_thread(",
        "pub async fn discard_change(",
    ] {
        let body = &src[src.find(entry).expect(entry)..];
        let body = &body[..body.find("\n    }\n").expect("end of fn")];
        assert!(
            body.contains("abandon_background_tasks("),
            "{entry} must kill the thread's background tasks"
        );
    }
}
