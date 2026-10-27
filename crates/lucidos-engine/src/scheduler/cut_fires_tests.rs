use super::*;
use crate::engine::event_bus::EventBus;
use sqlx::PgPool;

pub(crate) async fn report_all(pool: &PgPool, bus: &EventBus) {
    let queue = boot_queue(pool, bus);
    for fire in find_cut_fires(pool).await.unwrap() {
        let retry = prepare_retry(pool, &queue, Some(&test_config()), &fire)
            .await
            .unwrap();
        finish_report(pool, bus, &fire, retry).await;
        let notification: serde_json::Value = sqlx::query_scalar(
            "SELECT payload->'data' FROM events WHERE event_type = 'NotificationCreated' AND payload->'data'->>'thread_id' = $1"
        ).bind(fire.thread_id.to_string()).fetch_one(pool).await.unwrap();
        assert_eq!(notification["tap"]["to"]["id"], fire.thread_id.to_string());
        assert_eq!(notification["tap"]["to"]["target"], "thread");
        let (stamp, status): (String, String) = sqlx::query_as(
            "SELECT payload->>'last_run', payload->>'status' FROM events WHERE event_type = 'TriggerExecuted' AND payload->>'trigger_id' = $1 ORDER BY sequence DESC LIMIT 1"
        ).bind(&fire.trigger_id).fetch_one(pool).await.unwrap();
        assert_eq!(status, "failed");
        assert_eq!(
            stamp.parse::<chrono::DateTime<chrono::Utc>>().unwrap(),
            fire.started_at
        );
    }
}

fn boot_queue(pool: &PgPool, bus: &EventBus) -> Arc<ThreadQueue> {
    let queue = Arc::new(ThreadQueue::new(
        pool.clone(),
        bus.clone(),
        Arc::default(),
        std::path::PathBuf::new(),
        Arc::default(),
        crate::engine::thread_queue::CapacityPolicy::default(),
        Arc::default(),
    ));
    queue.begin_boot_recovery();
    queue
}

async fn finish_report(pool: &PgPool, bus: &EventBus, fire: &CutFire, retry: FireRetry) {
    if failure_stamp_needed(pool, fire).await.unwrap() {
        bus.record_trigger_executed_at(&fire.trigger_id, TriggerRunStatus::Failed, fire.started_at)
            .await
            .unwrap();
    }
    report_cut_fire(pool, bus, fire, retry, None).await.unwrap();
}

use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{
    emit_response_aborted, AbortCause, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;
use uuid::Uuid;

async fn emit(bus: &EventBus, thread: Uuid, kind: &str, payload: serde_json::Value) {
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::from_stored(kind, payload).unwrap(),
        meta: EventMeta {
            channel: Some(EventChannel::Trigger),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn start(bus: &EventBus, thread: Uuid, review: bool) {
    emit(
        bus,
        thread,
        "TriggerStarted",
        json!({
            "trigger_id": "cut-fire", "trigger_name": "Nightly report", "go_to_review": review
        }),
    )
    .await;
}

async fn abort(bus: &EventBus, thread: Uuid, cause: AbortCause, device: bool) {
    emit_response_aborted(
        bus,
        thread,
        cause,
        "Interrupted".into(),
        vec![],
        None,
        None,
        EventMeta {
            actor: Some(if device {
                MessageOrigin::Device {
                    device_id: "test-device".into(),
                }
            } else {
                MessageOrigin::system()
            }),
            ..EventMeta::NONE
        },
        "[Test] cut fire",
    )
    .await;
}

async fn state(pool: &PgPool, thread: Uuid) -> (String, String, bool) {
    sqlx::query_as(
        "SELECT status, archive_state, is_saved FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn negative(case: &str) {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start(&bus, thread, false).await;
    abort(
        &bus,
        thread,
        if case == "nonrestart" {
            AbortCause::StaleSettle
        } else {
            AbortCause::EngineShutdown
        },
        case == "switch" || case == "resumed",
    )
    .await;
    match case {
        "resumed" => emit(&bus, thread, "ResponseGenerated", json!({"text": "Resumed successfully"})).await,
        "failure" => emit(&bus, thread, "ResponseFailed", json!({"error": "Ordinary failure"})).await,
        "human" | "legacy-human" | "agent" => {
            let mut payload = json!({"text": "Follow-up", "mode": if case == "agent" { "agent" } else { "human" }});
            if case == "legacy-human" { payload.as_object_mut().unwrap().remove("mode"); }
            emit(&bus, thread, "MessageReceived", payload).await;
        }
        "child" => {
            let parent = Uuid::new_v4();
            start(&bus, parent, false).await;
            sqlx::query("UPDATE thread_summaries SET parent_thread_id = $2 WHERE thread_id = $1")
                .bind(thread).bind(parent).execute(&pool).await.unwrap();
        }
        "parked" => emit(&bus, thread, "UserQuestionAsked", json!({"tool_use_id": "question", "cc_session_id": "", "question": "Which report?", "options": [], "multi_select": false})).await,
        "old" => {
            // Test-only historical fixture; production timestamps remain bus-owned.
            sqlx::query("UPDATE events SET created = now() - make_interval(secs => $2) WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'")
                .bind(thread.to_string()).bind((CUT_FIRE_LOOKBACK_SECS + 1) as f64).execute(&pool).await.unwrap();
        }
        "script" => {
            sqlx::query("UPDATE thread_summaries SET source = 'chat' WHERE thread_id = $1")
                .bind(thread).execute(&pool).await.unwrap();
        }
        _ => {}
    }
    let before = state(&pool, thread).await;
    let fires = find_cut_fires(&pool).await.unwrap();
    if case == "agent" {
        assert_eq!(
            fires.len(),
            1,
            "automated follow-ups never make a fire attended"
        );
    } else {
        assert!(fires.is_empty(), "excluded population: {case}");
        report_all(&pool, &bus).await;
        assert_eq!(state(&pool, thread).await, before);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type IN ('TriggerRunInterrupted', 'NotificationCreated', 'TriggerExecuted')")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(count, 0);
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

macro_rules! negative_test {
    ($name:ident, $case:literal) => {
        #[tokio::test]
        async fn $name() {
            negative($case).await;
        }
    };
}
negative_test!(resumed_switch_is_untouched, "resumed");
negative_test!(promised_resume_is_untouched, "switch");
negative_test!(ordinary_failure_is_untouched, "failure");
negative_test!(nonrestart_abort_is_untouched, "nonrestart");
negative_test!(human_followup_is_untouched, "human");
negative_test!(legacy_human_followup_is_untouched, "legacy-human");
negative_test!(agent_followup_stays_unattended, "agent");
negative_test!(child_is_untouched, "child");
negative_test!(parked_thread_is_untouched, "parked");
negative_test!(old_abort_is_untouched, "old");
negative_test!(nontrigger_is_untouched, "script");

async fn positive(pin: bool, review: bool, completed: bool) {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start(&bus, thread, review).await;
    if pin {
        emit(&bus, thread, "ThreadSaved", json!({})).await;
    }
    if completed {
        emit(
            &bus,
            thread,
            "TriggerCompleted",
            json!({"trigger_id": "cut-fire"}),
        )
        .await;
        // A later engine-driven turn was cut after the fire did its own work.
        emit(
            &bus,
            thread,
            "MessageReceived",
            json!({"text": "Child report", "mode": "engine"}),
        )
        .await;
    }
    abort(&bus, thread, AbortCause::RecoveryAfterRestart, false).await;
    report_all(&pool, &bus).await;
    assert_eq!(
        state(&pool, thread).await,
        ("failed".into(), "inbox".into(), pin)
    );
    let retry: serde_json::Value = sqlx::query_scalar("SELECT payload->'retry' FROM events WHERE event_type = 'TriggerRunInterrupted' AND aggregate_id = $1")
        .bind(thread.to_string()).fetch_one(&pool).await.unwrap();
    assert_eq!(
        retry["reason"],
        if completed {
            "fire_completed"
        } else {
            "request_unavailable"
        }
    );
    report_all(&pool, &bus).await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type IN ('TriggerRunInterrupted', 'NotificationCreated', 'TriggerExecuted')")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(count, 3, "a second boot is a no-op");
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn pin_survives_reporting() {
    positive(true, false, false).await;
}
#[tokio::test]
async fn review_run_returns_to_inbox() {
    positive(false, true, false).await;
}
#[tokio::test]
async fn completed_fire_reports_without_retry() {
    positive(false, false, true).await;
}

#[test]
fn boot_reports_before_registration_and_catchup() {
    let source = include_str!("mod.rs");
    let start = source
        .split("pub async fn start(")
        .nth(1)
        .unwrap()
        .split("log!(\"[Scheduler] Started\")")
        .next()
        .unwrap();
    let replay = start
        .find("self.replay_triggers_from_events().await?")
        .unwrap();
    let report = start.find("cut_fires::settle_cut_trigger_fires(").unwrap();
    let register = start
        .find("self.register_triggers_from_configs().await?")
        .unwrap();
    assert!(replay < report && report < register);
    let boot = include_str!("../main.rs");
    assert!(
        boot.find(".begin_boot_recovery()").unwrap()
            < boot.find(".recover_persisted_entries()").unwrap()
    );
    assert!(
        boot.find("scheduler.start().await?").unwrap() < boot.find(".start_draining()").unwrap()
    );
}

#[test]
fn retry_eligibility_covers_every_rule_and_exact_deadline() {
    let eligible = RetryFacts {
        already_retried: false,
        completed: false,
        spawned_work: false,
        fresh: true,
        later_slot_due: false,
        newer_fire: false,
        available: true,
        request_available: true,
    };
    assert_eq!(decide_retry(eligible), FireRetryDecision::Submit);
    for (facts, reason) in [
        (
            RetryFacts {
                already_retried: true,
                ..eligible
            },
            RetrySkipReason::AlreadyRetried,
        ),
        (
            RetryFacts {
                completed: true,
                ..eligible
            },
            RetrySkipReason::FireCompleted,
        ),
        (
            RetryFacts {
                spawned_work: true,
                ..eligible
            },
            RetrySkipReason::SpawnedWork,
        ),
        (
            RetryFacts {
                fresh: false,
                ..eligible
            },
            RetrySkipReason::TooOld,
        ),
        (
            RetryFacts {
                later_slot_due: true,
                ..eligible
            },
            RetrySkipReason::LaterSlotDue,
        ),
        (
            RetryFacts {
                newer_fire: true,
                ..eligible
            },
            RetrySkipReason::NewerFire,
        ),
        (
            RetryFacts {
                available: false,
                ..eligible
            },
            RetrySkipReason::TriggerUnavailable,
        ),
        (
            RetryFacts {
                request_available: false,
                ..eligible
            },
            RetrySkipReason::RequestUnavailable,
        ),
    ] {
        assert_eq!(decide_retry(facts), FireRetryDecision::Skip(reason));
    }
}

#[test]
fn cron_deadline_uses_the_first_slot_after_the_cut_fire() {
    let at = "2026-10-25T03:00:00Z".parse().unwrap();
    let mut config = test_config();
    config.schedule = vec!["0 0 3 * * *".into()];
    assert_eq!(
        next_slot(&config, at).unwrap().unwrap(),
        at + chrono::Duration::days(1)
    );
    config.schedule = vec!["0 */15 * * * *".into()];
    assert_eq!(
        next_slot(&config, at).unwrap().unwrap(),
        at + chrono::Duration::minutes(15)
    );
}

fn test_config() -> crate::triggers::TriggerConfig {
    crate::triggers::TriggerConfig::from_created_payload(&json!({
        "trigger_id": "cut-fire", "name": "Nightly report",
        "run": {"type": "intent", "intent": "Prepare a report"}, "timezone": "UTC"
    }))
    .unwrap()
}

async fn seed_request(bus: &EventBus, thread: Uuid, request: ThreadQueueRequest) {
    use crate::engine::event_bus::SystemEvent;
    let entry_id = Uuid::new_v4();
    bus.emit(BusEvent::System(SystemEvent::ThreadQueued {
        entry_id,
        kind: request.kind(),
        trigger_id: Some("cut-fire".into()),
        trigger_name: None,
        thread_id: None,
        summary: "Report".into(),
        request: serde_json::to_value(request).unwrap(),
        requeued: false,
        actor: None,
    }))
    .await
    .unwrap();
    bus.emit(BusEvent::System(SystemEvent::ThreadQueueAdmitted {
        entry_id,
        thread_id: Some(thread),
        actor: None,
    }))
    .await
    .unwrap();
    bus.emit(BusEvent::System(SystemEvent::ThreadQueueCompleted {
        entry_id,
    }))
    .await
    .unwrap();
}

async fn cut_with_request(pool: &PgPool, bus: &EventBus, event: bool) -> CutFire {
    let thread = Uuid::new_v4();
    let request = if event {
        ThreadQueueRequest::EventTrigger {
            retry_of: None,
            trigger_id: "cut-fire".into(),
            event_type: "ReportRequested".into(),
            event_payload: json!({"summary": "Generate report", "count": 3}),
            depth: 2,
            origin_thread_id: Some(Uuid::new_v4()),
            source_event_id: Some(Uuid::new_v4()),
        }
    } else {
        ThreadQueueRequest::Cron {
            trigger_id: "cut-fire".into(),
            retry_of: None,
        }
    };
    seed_request(bus, thread, request).await;
    start(bus, thread, false).await;
    abort(bus, thread, AbortCause::RecoveryAfterRestart, false).await;
    find_cut_fires(pool).await.unwrap().pop().unwrap()
}

#[tokio::test]
async fn reporting_resumes_after_every_durable_step_without_duplicates() {
    for crash_after in 0..=4 {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let queue = boot_queue(&pool, &bus);
        let mut fire = cut_with_request(&pool, &bus, true).await;
        fire.trigger_name = "Long trigger name ".repeat(200);
        sqlx::query(
            "UPDATE events SET payload = jsonb_set(payload, '{trigger_name}', $2) WHERE id = $1",
        )
        .bind(fire.fire_event_id)
        .bind(json!(fire.trigger_name))
        .execute(&pool)
        .await
        .unwrap();
        let config = test_config();
        let original =
            serde_json::to_value(rebuild_request(&pool, &fire).await.unwrap().unwrap()).unwrap();
        if crash_after > 0 {
            let retry = prepare_retry(&pool, &queue, Some(&config), &fire)
                .await
                .unwrap();
            assert!(matches!(retry, FireRetry::Queued { .. }));
            if crash_after > 1 {
                bus.record_trigger_executed_at(
                    &fire.trigger_id,
                    TriggerRunStatus::Failed,
                    fire.started_at,
                )
                .await
                .unwrap();
            }
            if crash_after > 2 {
                notify_cut_fire(&pool, &bus, &fire, &retry).await.unwrap();
            }
            if crash_after > 3 {
                report_cut_fire(&pool, &bus, &fire, retry, None)
                    .await
                    .unwrap();
            }
        }
        // A fresh manager reconstructs its memory from the same durable queue.
        let next_queue = boot_queue(&pool, &bus);
        next_queue.recover_persisted_entries().await;
        for candidate in find_cut_fires(&pool).await.unwrap() {
            let retry = prepare_retry(&pool, &next_queue, Some(&config), &candidate)
                .await
                .unwrap();
            finish_report(&pool, &bus, &candidate, retry).await;
        }
        assert!(find_cut_fires(&pool).await.unwrap().is_empty());
        let counts: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT \
             (SELECT count(*) FROM events WHERE event_type = 'ThreadQueued' AND payload->'data'->'request' ? 'retry_of'), \
             (SELECT count(*) FROM events WHERE event_type = 'TriggerExecuted'), \
             (SELECT count(*) FROM events WHERE event_type = 'NotificationCreated'), \
             (SELECT count(*) FROM events WHERE event_type = 'TriggerRunInterrupted')"
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(counts, (1, 1, 1, 1), "crash after step {crash_after}");
        let queued: serde_json::Value =
            sqlx::query_scalar("SELECT request FROM thread_queue WHERE request ? 'retry_of'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let mut expected = original;
        expected["retry_of"] = queued["retry_of"].clone();
        assert_eq!(queued, expected, "event request context survives exactly");
        let status: String =
            sqlx::query_scalar("SELECT status FROM thread_queue WHERE request ? 'retry_of'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            status, "queued",
            "boot never admits or waits for completion"
        );
        pool.close().await;
        teardown_test_db(&db).await;
    }
}

#[tokio::test]
async fn cron_coalesces_both_before_and_after_retry_submission() {
    for existing in [false, true] {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let queue = boot_queue(&pool, &bus);
        let fire = cut_with_request(&pool, &bus, false).await;
        let request = || ThreadQueueRequest::Cron {
            trigger_id: "cut-fire".into(),
            retry_of: None,
        };
        if existing {
            assert!(queue.submit_boot_retry(request()).await.unwrap().is_some());
        }
        let retry = prepare_retry(&pool, &queue, Some(&test_config()), &fire)
            .await
            .unwrap();
        if existing {
            assert_eq!(
                retry,
                FireRetry::Skipped {
                    reason: RetrySkipReason::AlreadyQueued
                }
            );
        } else {
            assert!(matches!(retry, FireRetry::Queued { .. }));
            assert!(queue.submit_boot_retry(request()).await.unwrap().is_none());
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_queue")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        pool.close().await;
        teardown_test_db(&db).await;
    }
}

#[tokio::test]
async fn eligibility_reads_age_slot_children_and_newer_fire_from_database() {
    for case in [
        "daily",
        "quarter-hour",
        "exact-day",
        "over-day",
        "under-day",
        "newer",
        "children",
        "paused",
        "deleted",
        "retry",
    ] {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let queue = boot_queue(&pool, &bus);
        let mut fire = cut_with_request(&pool, &bus, false).await;
        let mut config = test_config();
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
            .fetch_one(&pool)
            .await
            .unwrap();
        let expected = match case {
            "daily" | "quarter-hour" => {
                let ago = now - chrono::Duration::hours(5) - chrono::Duration::minutes(30);
                config.schedule = if case == "daily" {
                    vec![ago.format("%S %M %H * * *").to_string()]
                } else {
                    vec!["0 */15 * * * *".into()]
                };
                fire.started_at = ago;
                if case == "daily" {
                    None
                } else {
                    Some(RetrySkipReason::LaterSlotDue)
                }
            }
            "exact-day" | "over-day" | "under-day" => {
                fire.started_at = now - chrono::Duration::seconds(FIRE_RETRY_WINDOW_SECS)
                    + chrono::Duration::seconds(if case == "under-day" {
                        60
                    } else if case == "over-day" {
                        -60
                    } else {
                        0
                    });
                if case == "under-day" {
                    None
                } else {
                    Some(RetrySkipReason::TooOld)
                }
            }
            "newer" => {
                start(&bus, Uuid::new_v4(), false).await;
                Some(RetrySkipReason::NewerFire)
            }
            "children" => {
                let child = Uuid::new_v4();
                start(&bus, child, false).await;
                sqlx::query(
                    "UPDATE thread_summaries SET parent_thread_id = $2 WHERE thread_id = $1",
                )
                .bind(child)
                .bind(fire.thread_id)
                .execute(&pool)
                .await
                .unwrap();
                Some(RetrySkipReason::SpawnedWork)
            }
            "paused" => {
                config.paused = true;
                Some(RetrySkipReason::TriggerUnavailable)
            }
            "deleted" => Some(RetrySkipReason::TriggerUnavailable),
            "retry" => {
                fire.already_retried = true;
                Some(RetrySkipReason::AlreadyRetried)
            }
            _ => unreachable!(),
        };
        let retry = prepare_retry(
            &pool,
            &queue,
            if case == "deleted" {
                None
            } else {
                Some(&config)
            },
            &fire,
        )
        .await
        .unwrap();
        match expected {
            Some(reason) => assert_eq!(retry, FireRetry::Skipped { reason }, "{case}"),
            None => {
                let FireRetry::Queued { deadline, .. } = retry else {
                    panic!("expected retry for {case}")
                };
                assert_eq!(
                    deadline,
                    (fire.started_at + chrono::Duration::seconds(FIRE_RETRY_WINDOW_SECS)).min(
                        next_slot(&config, fire.started_at)
                            .unwrap()
                            .unwrap_or(DateTime::<Utc>::MAX_UTC)
                    )
                );
            }
        }
        pool.close().await;
        teardown_test_db(&db).await;
    }
}

#[tokio::test]
async fn later_execution_stamp_is_never_overwritten_by_cut_reporting() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fire = cut_with_request(&pool, &bus, false).await;
    let later = fire.started_at + chrono::Duration::minutes(15);
    bus.record_trigger_executed_at(&fire.trigger_id, TriggerRunStatus::Ok, later)
        .await
        .unwrap();
    finish_report(
        &pool,
        &bus,
        &fire,
        FireRetry::Skipped {
            reason: RetrySkipReason::NewerFire,
        },
    )
    .await;
    let stamps: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT payload FROM events WHERE event_type = 'TriggerExecuted'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(stamps.len(), 1);
    assert_eq!(stamps[0]["status"], "ok");
    assert_eq!(
        stamps[0]["last_run"]
            .as_str()
            .unwrap()
            .parse::<DateTime<Utc>>()
            .unwrap(),
        later
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_cut_retry_is_reported_but_never_retried_across_more_boots() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let queue = boot_queue(&pool, &bus);
    let original = cut_with_request(&pool, &bus, true).await;
    let retry = prepare_retry(&pool, &queue, Some(&test_config()), &original)
        .await
        .unwrap();
    finish_report(&pool, &bus, &original, retry).await;
    let (entry_id, request_json): (Uuid, serde_json::Value) =
        sqlx::query_as("SELECT id, request FROM thread_queue")
            .fetch_one(&pool)
            .await
            .unwrap();
    let retry_thread = Uuid::new_v4();
    bus.emit(BusEvent::System(
        crate::engine::event_bus::SystemEvent::ThreadQueueAdmitted {
            entry_id,
            thread_id: Some(retry_thread),
            actor: None,
        },
    ))
    .await
    .unwrap();
    emit(
        &bus,
        retry_thread,
        "TriggerStarted",
        json!({
            "trigger_id": "cut-fire", "trigger_name": "Nightly report",
            "retry_of": request_json["retry_of"],
        }),
    )
    .await;
    abort(&bus, retry_thread, AbortCause::RecoveryAfterRestart, false).await;
    let retry_queued_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE event_type = 'ThreadQueued' AND payload->'data'->'request' ? 'retry_of'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(retry_queued_before, 1);
    for boot in 0..3 {
        let next = boot_queue(&pool, &bus);
        let fires = find_cut_fires(&pool).await.unwrap();
        assert_eq!(fires.len(), usize::from(boot == 0));
        for fire in fires {
            assert!(fire.already_retried);
            let outcome = prepare_retry(&pool, &next, Some(&test_config()), &fire)
                .await
                .unwrap();
            assert_eq!(
                outcome,
                FireRetry::Skipped {
                    reason: RetrySkipReason::AlreadyRetried
                }
            );
            finish_report(&pool, &bus, &fire, outcome).await;
        }
    }
    let retry_queued_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE event_type = 'ThreadQueued' AND payload->'data'->'request' ? 'retry_of'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(retry_queued_after, retry_queued_before);
    let notices: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'NotificationCreated'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(notices, 2);
    pool.close().await;
    teardown_test_db(&db).await;
}
