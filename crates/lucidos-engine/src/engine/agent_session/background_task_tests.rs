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

/// One `wait` must return inside the prompt cache's 5-minute life, whatever
/// the caller asks. A shorter ask is honoured.
#[test]
fn a_wait_is_capped_whatever_the_caller_asks() {
    assert_eq!(agent_wait_secs(None), AGENT_WAIT_MAX_SECS);
    assert_eq!(agent_wait_secs(Some(100_000)), AGENT_WAIT_MAX_SECS);
    assert_eq!(agent_wait_secs(Some(5)), 5);
    // A pin on the cache arithmetic, not a restated value: 300 s minus the
    // next request's own generation time.
    const { assert!(AGENT_WAIT_MAX_SECS <= 230) };
}

/// The CLI help, the CLI's own response deadline and the knowhow all state
/// the cap, and none can import it. A pin, so a changed cap fails here rather
/// than leaving them telling agents the old number.
#[test]
fn every_copy_of_the_wait_cap_states_the_engine_constant() {
    let cap = AGENT_WAIT_MAX_SECS;
    let copies = [
        (
            "crates/lucidos-cli/src/main.rs",
            include_str!("../../../../lucidos-cli/src/main.rs"),
        ),
        (
            "system-knowhow/lucidos-cli.md",
            include_str!("../../../../../system-knowhow/lucidos-cli.md"),
        ),
        (
            ".claude/skills/run-e2e/SKILL.md",
            include_str!("../../../../../.claude/skills/run-e2e/SKILL.md"),
        ),
        (
            ".claude/skills/run-tests/SKILL.md",
            include_str!("../../../../../.claude/skills/run-tests/SKILL.md"),
        ),
    ];
    for (path, text) in copies {
        assert!(
            text.contains(&format!("{cap} s")),
            "{path} must state the {cap} s cap"
        );
    }
    let cli = include_str!("../../../../lucidos-cli/src/background_task.rs");
    let deadline: u64 = cli
        .split("WAIT_RESPONSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|n| n.parse().ok())
        .expect("the CLI's wait deadline");
    assert!(
        deadline > cap,
        "the CLI must wait longer than the engine's cap"
    );
}

/// A coding agent's shell holds none of the workspace's secrets. A task it
/// starts through the engine must not either, or the route hands them over:
/// the chat tool's env carries each `CRED_*` and `OAUTH_*` value its text names.
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
        "pub async fn discard_open_changes_for_thread(",
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

/// An archive lost to a broadcast lag must still kill the thread's task, or it
/// runs on to its own watchdog.
#[tokio::test]
async fn an_archive_lost_to_a_lag_still_kills_the_threads_task() {
    use crate::engine::event_bus::EventBus;
    use crate::engine::thread_events::ThreadEvent;
    use crate::test_support::{
        flood_past_bus_capacity, seed_chat_thread, seed_thread_event, setup_test_db,
        teardown_test_db,
    };

    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let registry = BackgroundBashRegistry::new();
    let thread_id = seed_chat_thread(&bus, "run the suite").await;
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::ResponseGenerated {
            text: "started".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;
    let (task_id, _rx) = registry
        .spawn(
            "sleep 30",
            60,
            std::path::Path::new("/tmp"),
            &[],
            Some(thread_id),
            None,
        )
        .await
        .expect("spawn");
    let (mut rx, mut replay) = LagReplay::subscribe(
        &bus,
        ReplayScope::Workspace,
        Some(WORK_ENDING_EVENT_TYPES.to_vec()),
    );

    seed_thread_event(&bus, thread_id, ThreadEvent::ThreadArchived).await;
    flood_past_bus_capacity(&bus);

    assert!(reap_ended_work(rx.recv().await, &mut replay, &registry, &pool).await);
    // A second kill finds nothing left to signal only if the reaper sent the
    // stop. Otherwise it stops the task here, so the test leaves no process.
    assert_eq!(
        registry.kill_for_thread(thread_id).await,
        0,
        "the archived thread's task {task_id} must already be stopped"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// An unattended trigger thread is archived by its terminator, with no
/// `ThreadArchived`, while its background task may still run. The live path
/// leaves that task alone, so a lag must too.
#[tokio::test]
async fn a_lag_leaves_a_task_running_on_a_thread_archived_without_an_archive_event() {
    use crate::engine::event_bus::EventBus;
    use crate::test_support::{
        flood_past_bus_capacity, seed_chat_thread, setup_test_db, teardown_test_db,
    };

    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let registry = BackgroundBashRegistry::new();
    let thread_id = seed_chat_thread(&bus, "nightly sync").await;
    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();
    registry
        .spawn(
            "sleep 30",
            60,
            std::path::Path::new("/tmp"),
            &[],
            Some(thread_id),
            None,
        )
        .await
        .expect("spawn");
    let (mut rx, mut replay) = LagReplay::subscribe(
        &bus,
        ReplayScope::Workspace,
        Some(WORK_ENDING_EVENT_TYPES.to_vec()),
    );
    flood_past_bus_capacity(&bus);

    assert!(reap_ended_work(rx.recv().await, &mut replay, &registry, &pool).await);
    assert_eq!(
        registry.kill_for_thread(thread_id).await,
        1,
        "the task must still be running, so this cleanup is what stops it"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The replay list gates live frames too, so an event the reaper acts on but
/// the list omits would never reach it.
#[test]
fn the_reaper_replays_every_event_that_ends_a_threads_work() {
    use crate::engine::thread_events::ThreadEvent;
    let ending = [
        ThreadEvent::ThreadArchived,
        ThreadEvent::ThreadDiscarded { actor: None },
    ];
    for event in &ending {
        assert!(ends_the_threads_work(event));
        assert!(WORK_ENDING_EVENT_TYPES.contains(&event.event_type()));
    }
    assert_eq!(WORK_ENDING_EVENT_TYPES.len(), ending.len());
}
