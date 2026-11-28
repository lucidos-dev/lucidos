use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::event_bus::EventBus;
use crate::engine::event_wait::{catch_up_from_watermark, LiveWait};
use crate::engine::thread_events::ActorMode;
use crate::test_support::{seed_thread_event, setup_test_db, teardown_test_db};

async fn seed_thread(bus: &EventBus, thread_id: Uuid) {
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "kick off the release".into(),
            user_image_hashes: vec![],
            device_id: None,
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
        background_timing: None,
        wait_id: Uuid::new_v4(),
        thread_id,
        tool_use_id: "engine:bg-task-wait:1".into(),
        on: vec![EventSubscription {
            event_type: "BackgroundBashCompleted".into(),
            condition: Some(serde_json::json!({ "task_id": "task-watched" })),
        }],
        reason: "the background command \"sleep 300\" to finish".into(),
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
        background_timing: None,
        wait_id: Uuid::new_v4(),
        thread_id,
        tool_use_id: "engine:bg-task-wait:2".into(),
        on: vec![EventSubscription {
            event_type: "BackgroundBashCompleted".into(),
            condition: Some(serde_json::json!({ "task_id": "the-other-one" })),
        }],
        reason: "the background command \"sleep 300\" to finish".into(),
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

#[tokio::test]
async fn a_task_this_process_is_running_is_not_settleable() {
    let reg = crate::engine::bash_background::BackgroundBashRegistry::new();
    let (live, _rx) = reg
        .spawn(
            "sleep 300",
            600,
            std::path::Path::new("/tmp"),
            &[],
            None,
            None,
        )
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

#[tokio::test]
async fn a_finished_task_in_the_registry_is_still_settleable() {
    let reg = crate::engine::bash_background::BackgroundBashRegistry::new();
    let (done, _rx) = reg
        .spawn(
            "echo done",
            5,
            std::path::Path::new("/tmp"),
            &[],
            None,
            None,
        )
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

#[test]
fn neither_note_promises_the_work_stopped() {
    assert!(ENGINE_STOPPED_NOTE.contains("it killed this task"));
    assert!(
        ENGINE_STOPPED_NOTE.contains("may still be running"),
        "a group kill cannot reach a detached session: {ENGINE_STOPPED_NOTE}"
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
