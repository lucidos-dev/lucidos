use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::event_bus::EventBus;
use crate::engine::event_wait::{catch_up_from_watermark, LiveWait};
use crate::engine::thread_events::ActorMode;
use crate::test_support::{seed_thread_event, setup_test_db, teardown_test_db};

/// A thread the projection knows about. The sweep inner-joins
/// `thread_summaries`, and `BackgroundBashStarted` creates no row of its own.
async fn seed_thread(bus: &EventBus, thread_id: Uuid) {
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: "kick off the release".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
    )
    .await;
}

/// Spawn a background task, as the event store records it.
///
/// `timeout_secs` doubles as the watchdog bound the sweep tests against: a `0`
/// budget puts the row past its own deadline the instant it is written, with no
/// clock to manipulate and nothing to sleep through.
async fn seed_started(
    bus: &EventBus,
    thread_id: Uuid,
    task_id: &str,
    timeout_secs: u64,
) -> DateTime<Utc> {
    let started_at = Utc::now();
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::BackgroundBashStarted {
            task_id: task_id.to_string(),
            command: "./scripts/release.sh -y 0.31.0".into(),
            timeout_secs,
            started_at,
        },
    )
    .await;
    started_at
}

/// The lookup `execute_bash_output_tool` falls back to when the registry has
/// nothing. Written out rather than called, because the point of the assertion
/// is that the emitted row satisfies those exact predicates.
async fn bash_output_fallback_finds(pool: &sqlx::PgPool, thread_id: Uuid, task_id: &str) -> bool {
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT payload FROM events \
         WHERE event_type = 'BackgroundBashCompleted' \
           AND aggregate_id = $1 \
           AND payload->>'task_id' = $2 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .bind(task_id)
    .fetch_optional(pool)
    .await
    .unwrap();
    row.is_some()
}

// ── selection ───────────────────────────────────────────────────────

/// THE regression, as the event store shows it. A release thread spawned a
/// watcher, the engine restarted 15 minutes in, and that `task_id` kept a
/// `BackgroundBashStarted` row and nothing else forever.
#[tokio::test]
async fn a_started_task_with_no_completion_is_abandoned() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    let started_at = seed_started(&bus, thread_id, "task-alive", 3600).await;

    let found = abandoned_background_tasks(&pool).await.unwrap();
    assert_eq!(found.len(), 1, "found: {found:?}");
    assert_eq!(found[0].thread_id, thread_id);
    assert_eq!(found[0].task_id, "task-alive");
    assert_eq!(found[0].command, "./scripts/release.sh -y 0.31.0");
    // Read off the payload, not synthesized, so the settled event reports the
    // task's real start rather than the boot it was discovered at.
    assert_eq!(
        found[0].started_at.timestamp_millis(),
        started_at.timestamp_millis(),
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_task_that_reached_its_completion_is_not_abandoned() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    let started_at = seed_started(&bus, thread_id, "task-done", 3600).await;
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::BackgroundBashCompleted {
            task_id: "task-done".into(),
            command: "./scripts/release.sh -y 0.31.0".into(),
            exit_code: Some(0),
            signal: None,
            stdout: "done".into(),
            stderr: String::new(),
            started_at,
            finished_at: Utc::now(),
            timed_out: false,
            killed: false,
            abandoned: false,
        },
    )
    .await;

    assert!(abandoned_background_tasks(&pool).await.unwrap().is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// **A task past its own watchdog deadline is settled too**, and the ordering
/// pins it: `task-expired` was written with a zero budget, so it is already
/// past its deadline when the sweep looks.
///
/// An earlier draft bounded the sweep at that deadline, to keep the first boot
/// after this shipped from writing rows onto long-finished threads. It bought
/// a one-time tidiness with a permanent hole: an engine down longer than the
/// budget left the task unsettled forever, and its wait then expired blaming
/// the deadline. That is the stall the whole module removes.
#[tokio::test]
async fn a_task_past_its_own_watchdog_deadline_is_settled_too() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    seed_started(&bus, thread_id, "task-expired", 0).await;
    seed_started(&bus, thread_id, "task-live", 3600).await;

    let found = abandoned_background_tasks(&pool).await.unwrap();
    let ids: Vec<&str> = found.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(ids, vec!["task-expired", "task-live"], "found: {found:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Same exclusion as `lost_wait_reentries` and
/// `settle_legacy_attached_event_waits`. Reviving a thread the user threw away
/// is the archive-curtain problem in another costume.
#[tokio::test]
async fn a_discarded_threads_task_is_left_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    seed_started(&bus, thread_id, "task-discarded", 3600).await;
    sqlx::query("UPDATE thread_summaries SET state = 'discarded' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();

    assert!(abandoned_background_tasks(&pool).await.unwrap().is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ── what the settled event buys ─────────────────────────────────────

/// The two things the emit has to earn, plus the guard against doing it twice.
///
/// `bash_output`'s persisted fallback binds `aggregate_id` to the thread and
/// `payload->>'task_id'` to the id. A row missing either is one the drain still
/// calls `unknown task_id`. And the anti-join is the whole idempotency story
/// between the boot sweep and the teardown emit: once the completion exists, a
/// second pass must see nothing.
#[tokio::test]
async fn the_settled_completion_is_found_by_the_drain_and_settles_the_task() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    let started_at = seed_started(&bus, thread_id, "task-settled", 3600).await;
    assert!(
        !bash_output_fallback_finds(&pool, thread_id, "task-settled").await,
        "precondition: the drain finds nothing before the sweep, which is the bug",
    );

    seed_thread_event(
        &bus,
        thread_id,
        abandoned_completion(
            "task-settled".into(),
            "./scripts/release.sh -y 0.31.0".into(),
            started_at,
            ENGINE_CRASHED_NOTE,
        ),
    )
    .await;

    assert!(bash_output_fallback_finds(&pool, thread_id, "task-settled").await);
    assert!(
        abandoned_background_tasks(&pool).await.unwrap().is_empty(),
        "a settled task must not be settled again",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The stall itself. A wait armed by `arm_wait_for_running_background_tasks`
/// is conditioned on `BackgroundBashCompleted{task_id}`, and until the sweep
/// existed nothing could ever satisfy it. The boot rebuild runs this exact
/// catch-up scan over every wait it re-derives. A match here is the thread
/// re-opening at boot rather than at its deadline.
#[tokio::test]
async fn the_settled_completion_resolves_the_wait_armed_on_that_task() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    let started_at = seed_started(&bus, thread_id, "task-watched", 3600).await;

    let watermark: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    let wait = LiveWait {
        wait_id: Uuid::new_v4(),
        thread_id,
        tool_use_id: "engine:bg-task-wait:1".into(),
        on: vec![EventSubscription {
            event_type: "BackgroundBashCompleted".into(),
            condition: Some(serde_json::json!({ "task_id": "task-watched" })),
        }],
        reason: "Watching background work started in this thread".into(),
        armed_at: Utc::now(),
        expires_at: Utc::now() + chrono::Duration::hours(1),
        watermark,
    };
    assert!(
        catch_up_from_watermark(&pool, &wait)
            .await
            .unwrap()
            .is_none(),
        "precondition: nothing satisfies the wait while the task is unsettled",
    );

    seed_thread_event(
        &bus,
        thread_id,
        abandoned_completion(
            "task-watched".into(),
            "./scripts/release.sh -y 0.31.0".into(),
            started_at,
            ENGINE_CRASHED_NOTE,
        ),
    )
    .await;

    let matched = catch_up_from_watermark(&pool, &wait).await.unwrap();
    let (_, event_type, payload, index) = matched.expect("the settled completion must deliver");
    assert_eq!(event_type, "BackgroundBashCompleted");
    assert_eq!(index, 0);
    assert_eq!(payload["abandoned"], serde_json::json!(true));
    assert_eq!(
        payload["killed"],
        serde_json::Value::Null,
        "`killed` means bash_kill, so an abandoned task must not claim it",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A wait scoped to a DIFFERENT task must not be spent by this one. The whole
/// reason the engine conditions on `task_id` is that an unconditioned
/// subscription fires on any background task finishing anywhere.
#[tokio::test]
async fn a_settled_completion_does_not_resolve_another_tasks_wait() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    let watermark: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    let wait = LiveWait {
        wait_id: Uuid::new_v4(),
        thread_id,
        tool_use_id: "engine:bg-task-wait:2".into(),
        on: vec![EventSubscription {
            event_type: "BackgroundBashCompleted".into(),
            condition: Some(serde_json::json!({ "task_id": "the-other-one" })),
        }],
        reason: "Watching background work started in this thread".into(),
        armed_at: Utc::now(),
        expires_at: Utc::now() + chrono::Duration::hours(1),
        watermark,
    };

    seed_thread_event(
        &bus,
        thread_id,
        abandoned_completion(
            "task-watched".into(),
            "cargo build".into(),
            Utc::now(),
            ENGINE_CRASHED_NOTE,
        ),
    )
    .await;

    assert!(catch_up_from_watermark(&pool, &wait)
        .await
        .unwrap()
        .is_none());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ── the live-task guard ─────────────────────────────────────────────

/// **The sweep must never settle a task this process is still running.** Doing
/// so resolves its wait mid-flight and makes the next drain report a finished
/// task that is still building.
///
/// Unreachable at boot, where the registry starts empty. So it is tested here
/// rather than left inline in the engine method. There it could be deleted
/// with the whole suite still green.
#[tokio::test]
async fn a_task_this_process_is_running_is_not_settleable() {
    let reg = crate::engine::tools::bash_background::BackgroundBashRegistry::new();
    let (live, _rx) = reg
        .spawn("sleep 300", 600, std::path::Path::new("/tmp"), &[], None)
        .await
        .expect("spawn");

    let candidates = vec![
        AbandonedTask {
            thread_id: Uuid::new_v4(),
            task_id: live.clone(),
            command: "sleep 300".into(),
            started_at: Utc::now(),
        },
        AbandonedTask {
            thread_id: Uuid::new_v4(),
            task_id: "gone-with-the-last-engine".into(),
            command: "./scripts/release.sh".into(),
            started_at: Utc::now(),
        },
    ];

    let keep = settleable(&reg, candidates).await;
    let ids: Vec<&str> = keep.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(ids, vec!["gone-with-the-last-engine"], "keep: {keep:?}");

    reg.kill(&live).await;
}

/// A task the registry holds but has FINISHED is settleable. It is not running,
/// so the guard has nothing to protect, and its own watcher may never have run.
#[tokio::test]
async fn a_finished_task_in_the_registry_is_still_settleable() {
    let reg = crate::engine::tools::bash_background::BackgroundBashRegistry::new();
    let (done, _rx) = reg
        .spawn("echo done", 5, std::path::Path::new("/tmp"), &[], None)
        .await
        .expect("spawn");
    assert!(
        reg.wait_until_finished(&done, std::time::Duration::from_secs(8))
            .await
    );

    let candidates = vec![AbandonedTask {
        thread_id: Uuid::new_v4(),
        task_id: done.clone(),
        command: "echo done".into(),
        started_at: Utc::now(),
    }];
    assert_eq!(settleable(&reg, candidates).await.len(), 1);
}

// ── the event's own shape ───────────────────────────────────────────

/// No status was reaped, so none is invented. `exit_code: 0` on a task the
/// engine killed would read as a clean success to every consumer.
#[test]
fn an_abandoned_completion_claims_no_exit_status() {
    let event = abandoned_completion(
        "t".into(),
        "cargo build".into(),
        Utc::now(),
        ENGINE_CRASHED_NOTE,
    );
    let ThreadEvent::BackgroundBashCompleted {
        exit_code,
        signal,
        timed_out,
        killed,
        abandoned,
        stderr,
        ..
    } = event
    else {
        panic!("expected a completion");
    };
    assert_eq!(exit_code, None);
    assert_eq!(signal, None);
    assert!(
        !timed_out,
        "the watchdog did not fire; the engine went away"
    );
    assert!(!killed, "`killed` means bash_kill, which nobody called");
    assert!(abandoned);
    assert_eq!(stderr, ENGINE_CRASHED_NOTE);
}

/// **Neither note promises the work stopped**, and that restraint is the
/// point. After a crash no destructor ran at all. After a teardown the kill
/// reached the `bash -c` wrapper, but a pipeline or a list leaves its real work
/// reparented to init. Telling an agent "nothing is running, re-run it" in
/// either case starts a second release beside the first.
#[test]
fn neither_note_promises_the_work_stopped() {
    // Neither promises the work stopped, and for different reasons. The crash
    // path ran no destructor at all; the teardown's SIGKILL reaches the `bash
    // -c` wrapper but not the pipeline behind it.
    assert!(ENGINE_STOPPED_NOTE.contains("it killed this task"));
    assert!(
        ENGINE_STOPPED_NOTE.contains("may still be running"),
        "a single-pid kill cannot promise the pipeline stopped: {ENGINE_STOPPED_NOTE}"
    );
    assert!(
        !ENGINE_CRASHED_NOTE.contains("killed"),
        "the crash path killed nothing: {ENGINE_CRASHED_NOTE}"
    );
    assert!(ENGINE_CRASHED_NOTE.contains("may or may not have outlived"));
}

#[test]
fn the_note_follows_output_that_did_arrive() {
    assert_eq!(
        with_note("warning: unused\n".into(), ENGINE_STOPPED_NOTE),
        format!("warning: unused\n\n{ENGINE_STOPPED_NOTE}"),
    );
    assert_eq!(
        with_note(String::new(), ENGINE_STOPPED_NOTE),
        ENGINE_STOPPED_NOTE,
        "an empty stream must not gain a leading blank line",
    );
}
