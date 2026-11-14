use super::*;
use crate::test_support::{seed_trigger_event, setup_test_db, teardown_test_db};

fn utc(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn cron(exprs: &[&str]) -> Vec<cron::Schedule> {
    exprs
        .iter()
        .map(|e| crate::engine::tools::scheduler::parse_standard_cron(e).unwrap())
        .collect()
}

const OSLO: chrono_tz::Tz = chrono_tz::Europe::Oslo;
const GRACE: Duration = Duration::minutes(super::super::MISSED_TASK_GRACE_MINUTES);

// ── The planner ─────────────────────────────────────────────────────────

#[test]
fn a_daily_slot_the_mac_slept_through_is_reported() {
    // 07:00 Oslo is 06:00 UTC in November. Opened at 09:00, two hours late.
    let plan = plan_missed_slots(
        &cron(&["0 0 7 * * *"]),
        OSLO,
        utc("2026-11-02T06:00:05Z"),
        utc("2026-11-03T08:00:00Z"),
        GRACE,
    );
    let at = utc("2026-11-03T06:00:00Z");
    assert_eq!(
        plan,
        SlotPlan::Report(SkippedSlots {
            first: at,
            last: at,
            count: 1
        })
    );
}

#[test]
fn a_poller_after_a_night_asleep_catches_up_once_and_reports_nothing() {
    let plan = plan_missed_slots(
        &cron(&["0 */15 * * * *"]),
        OSLO,
        utc("2026-11-02T22:00:00Z"),
        utc("2026-11-03T07:05:00Z"),
        GRACE,
    );
    assert_eq!(plan, SlotPlan::CatchUp(utc("2026-11-03T07:00:00Z")));
}

#[test]
fn a_gap_of_several_days_is_one_report() {
    let plan = plan_missed_slots(
        &cron(&["0 0 7 * * *"]),
        OSLO,
        utc("2026-10-31T06:00:05Z"),
        utc("2026-11-03T08:00:00Z"),
        GRACE,
    );
    assert_eq!(
        plan,
        SlotPlan::Report(SkippedSlots {
            first: utc("2026-11-01T06:00:00Z"),
            last: utc("2026-11-03T06:00:00Z"),
            count: 3
        })
    );
}

#[test]
fn two_expressions_on_one_instant_count_it_once() {
    // Tuesday 3 November is a weekday, so both expressions name 07:00.
    let plan = plan_missed_slots(
        &cron(&["0 0 7 * * *", "0 0 7 * * 1-5"]),
        OSLO,
        utc("2026-11-03T05:00:00Z"),
        utc("2026-11-03T08:00:00Z"),
        GRACE,
    );
    let at = utc("2026-11-03T06:00:00Z");
    assert_eq!(
        plan,
        SlotPlan::Report(SkippedSlots {
            first: at,
            last: at,
            count: 1
        })
    );
}

#[test]
fn interleaved_expressions_merge_in_order() {
    let plan = plan_missed_slots(
        &cron(&["0 0 19 * * *", "0 0 7 * * *", "0 0 7 * * 1-5"]),
        OSLO,
        utc("2026-11-01T12:00:00Z"),
        utc("2026-11-03T08:00:00Z"),
        GRACE,
    );
    // 19:00 on the 1st and 2nd, 07:00 on the 2nd and 3rd.
    assert_eq!(
        plan,
        SlotPlan::Report(SkippedSlots {
            first: utc("2026-11-01T18:00:00Z"),
            last: utc("2026-11-03T06:00:00Z"),
            count: 4
        })
    );
}

#[test]
fn a_per_second_schedule_counts_every_slot_of_the_gap() {
    let floor = utc("2026-11-02T00:00:00Z");
    let plan = plan_missed_slots(
        &cron(&["* * * * * *"]),
        OSLO,
        floor,
        floor + Duration::days(1) + GRACE,
        Duration::zero(),
    );
    let last = floor + Duration::days(1) + GRACE;
    assert_eq!(
        plan,
        SlotPlan::Report(SkippedSlots {
            first: floor + Duration::seconds(1),
            last,
            count: u32::try_from((last - floor).num_seconds()).unwrap()
        })
    );
}

#[test]
fn the_floor_itself_is_not_a_missed_slot_and_now_is() {
    let slot = utc("2026-11-03T06:00:00Z");
    let schedules = cron(&["0 0 7 * * *"]);
    assert_eq!(
        plan_missed_slots(&schedules, OSLO, slot, slot + Duration::hours(2), GRACE),
        SlotPlan::Nothing
    );
    assert_eq!(
        plan_missed_slots(&schedules, OSLO, slot - Duration::seconds(1), slot, GRACE),
        SlotPlan::CatchUp(slot)
    );
}

#[test]
fn a_slot_exactly_one_grace_old_is_reported_not_fired() {
    // The old startup catch-up fired only on `delay < grace`.
    let slot = utc("2026-11-03T06:00:00Z");
    assert_eq!(
        plan_missed_slots(
            &cron(&["0 0 7 * * *"]),
            OSLO,
            slot - Duration::hours(1),
            slot + GRACE,
            GRACE,
        ),
        SlotPlan::Report(SkippedSlots {
            first: slot,
            last: slot,
            count: 1
        })
    );
}

// ── The floor ───────────────────────────────────────────────────────────

const NOW: &str = "2026-11-03T08:00:00Z";

#[test]
fn with_nothing_known_the_floor_is_the_lookback() {
    let now = utc(NOW);
    assert_eq!(
        SlotFloors::default().floor(None, now),
        now - Duration::seconds(super::super::cut_fires::BOOT_REPORT_LOOKBACK_SECS)
    );
}

#[test]
fn the_floor_never_reaches_past_the_lookback() {
    let now = utc(NOW);
    let ancient = Some(utc("2026-01-01T00:00:00Z"));
    let floors = SlotFloors {
        last_run: ancient,
        created: ancient,
        config_changed: ancient,
        last_reported: ancient,
        restored_at: ancient,
    };
    assert_eq!(
        floors.floor(ancient, now),
        now - Duration::seconds(super::super::cut_fires::BOOT_REPORT_LOOKBACK_SECS)
    );
}

#[test]
fn every_known_instant_can_raise_the_floor() {
    let now = utc(NOW);
    let late = utc("2026-11-03T05:00:00Z");
    let early = Some(utc("2026-11-02T05:00:00Z"));
    let base = SlotFloors {
        last_run: early,
        created: early,
        config_changed: early,
        last_reported: early,
        restored_at: early,
    };
    let cases: [(&str, SlotFloors); 5] = [
        (
            "the last run",
            SlotFloors {
                last_run: Some(late),
                ..base
            },
        ),
        (
            "the creation",
            SlotFloors {
                created: Some(late),
                ..base
            },
        ),
        (
            "a pause, resume or edit",
            SlotFloors {
                config_changed: Some(late),
                ..base
            },
        ),
        (
            "an earlier report",
            SlotFloors {
                last_reported: Some(late),
                ..base
            },
        ),
        (
            "a restore",
            SlotFloors {
                restored_at: Some(late),
                ..base
            },
        ),
    ];
    for (what, floors) in cases {
        assert_eq!(floors.floor(None, now), late, "{what} raises the floor");
    }
    assert_eq!(
        base.floor(Some(late), now),
        late,
        "the caller's floor counts too"
    );
}

#[test]
fn a_resumed_trigger_reports_nothing_from_its_paused_period() {
    let now = utc(NOW);
    let floors = SlotFloors {
        last_run: Some(utc("2026-10-30T06:00:04Z")),
        config_changed: Some(utc("2026-11-03T07:55:00Z")),
        ..SlotFloors::default()
    };
    assert_eq!(
        plan_missed_slots(
            &cron(&["0 0 7 * * *"]),
            OSLO,
            floors.floor(None, now),
            now,
            GRACE
        ),
        SlotPlan::Nothing
    );
}

#[test]
fn a_late_wake_after_a_recorded_run_settles_nothing() {
    // The slot ran (late) before the runner noticed it overslept.
    let slot = utc("2026-11-03T06:00:00Z");
    let now = slot + Duration::hours(2);
    let floors = SlotFloors {
        last_run: Some(slot + Duration::seconds(4)),
        ..SlotFloors::default()
    };
    let floor = floors.floor(Some(slot - Duration::nanoseconds(1)), now);
    assert_eq!(
        plan_missed_slots(&cron(&["0 0 7 * * *"]), OSLO, floor, now, GRACE),
        SlotPlan::Nothing
    );
}

// ── The notice ──────────────────────────────────────────────────────────

#[test]
fn the_notice_names_the_trigger_and_the_slots_in_its_timezone() {
    let one = SkippedSlots {
        first: utc("2026-11-03T06:00:00Z"),
        last: utc("2026-11-03T06:00:00Z"),
        count: 1,
    };
    let (title, body) = notice_text("Morning brief", &one, OSLO);
    assert_eq!(title, "Morning brief: scheduled run skipped");
    assert!(body.contains("Tue 3 Nov 07:00"), "{body}");

    let five = SkippedSlots {
        first: utc("2026-10-30T06:00:00Z"),
        count: 5,
        ..one
    };
    let (title, body) = notice_text("Morning brief", &five, OSLO);
    assert_eq!(title, "Morning brief: 5 scheduled runs skipped");
    assert!(body.contains("Fri 30 Oct 07:00"), "{body}");
    assert!(body.contains("Tue 3 Nov 07:00"), "{body}");
}

// ── Against the store ───────────────────────────────────────────────────

const TRIGGER: &str = "99999999-9999-9999-9999-999999999999";

#[tokio::test]
async fn the_floors_come_from_the_trigger_history_and_the_restore_boundary() {
    let (pool, db) = setup_test_db().await;
    let seed = |event: &'static str, at: &'static str, payload: serde_json::Value| {
        let pool = pool.clone();
        async move { seed_trigger_event(&pool, TRIGGER, event, at, payload).await }
    };
    seed(
        "TriggerCreated",
        "2026-10-01T09:00:00Z",
        serde_json::json!({ "trigger_id": TRIGGER }),
    )
    .await;
    seed(
        "TriggerUpdated",
        "2026-10-02T09:00:00Z",
        serde_json::json!({ "trigger_id": TRIGGER }),
    )
    .await;
    seed(
        "TriggerDisabled",
        "2026-10-03T09:00:00Z",
        serde_json::json!({ "trigger_id": TRIGGER }),
    )
    .await;
    seed(
        "TriggerEnabled",
        "2026-10-04T09:00:00Z",
        serde_json::json!({ "trigger_id": TRIGGER }),
    )
    .await;
    seed(
        "TriggerExecuted",
        "2026-10-05T06:00:01Z",
        serde_json::json!({ "trigger_id": TRIGGER, "last_run": "2026-10-05T06:00:04Z" }),
    )
    .await;
    seed(
        "TriggerSlotsSkipped",
        "2026-10-08T07:00:00Z",
        serde_json::json!({ "type": "TriggerSlotsSkipped", "data": { "last_slot": "2026-10-07T06:00:00Z" } }),
    )
    .await;
    // Another trigger's history never leaks in.
    seed_trigger_event(
        &pool,
        "other",
        "TriggerEnabled",
        "2026-10-30T09:00:00Z",
        serde_json::json!({ "trigger_id": "other" }),
    )
    .await;

    let floors = load_slot_floors(&pool, TRIGGER, None).await.unwrap();
    assert_eq!(
        floors,
        SlotFloors {
            last_run: Some(utc("2026-10-05T06:00:04Z")),
            created: Some(utc("2026-10-01T09:00:00Z")),
            config_changed: Some(utc("2026-10-04T09:00:00Z")),
            last_reported: Some(utc("2026-10-07T06:00:00Z")),
            restored_at: None,
        }
    );

    // The in-memory run counts when it is the later one.
    let later = utc("2026-10-06T06:00:02Z");
    assert_eq!(
        load_slot_floors(&pool, TRIGGER, Some(later))
            .await
            .unwrap()
            .last_run,
        Some(later)
    );

    sqlx::query(
        "INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) \
         VALUES ($1, $2, $3, 0)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(crate::core::snapshot::handle::Handle::generate().as_str())
    .bind(utc("2026-10-20T12:00:00Z"))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        load_slot_floors(&pool, TRIGGER, None)
            .await
            .unwrap()
            .restored_at,
        Some(utc("2026-10-20T12:00:00Z"))
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_reported_gap_is_reported_once_across_restarts() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_trigger_event(
        &pool,
        TRIGGER,
        "TriggerExecuted",
        "2026-11-02T06:00:01Z",
        serde_json::json!({ "trigger_id": TRIGGER, "last_run": "2026-11-02T06:00:01Z" }),
    )
    .await;
    let schedules = cron(&["0 0 7 * * *"]);
    let now = utc(NOW);

    // Two boots over the same gap.
    for _ in 0..2 {
        let floors = load_slot_floors(&pool, TRIGGER, None).await.unwrap();
        if let SlotPlan::Report(slots) =
            plan_missed_slots(&schedules, OSLO, floors.floor(None, now), now, GRACE)
        {
            report_skipped_slots(&bus, TRIGGER, "Morning brief", OSLO, slots)
                .await
                .unwrap();
        }
    }

    let reports: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'data' FROM events WHERE event_type = 'TriggerSlotsSkipped' \
         AND aggregate = 'trigger' AND aggregate_id = $1",
    )
    .bind(TRIGGER)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0]["trigger_name"], "Morning brief");
    assert_eq!(reports[0]["count"], 1);
    assert_eq!(
        reports[0]["last_slot"]
            .as_str()
            .unwrap()
            .parse::<DateTime<Utc>>()
            .unwrap(),
        utc("2026-11-03T06:00:00Z")
    );

    let notices: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'data' FROM events WHERE event_type = 'NotificationCreated'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0]["title"], "Morning brief: scheduled run skipped");
    assert_eq!(notices[0]["task_id"], TRIGGER);
    assert_eq!(notices[0]["tap"]["to"]["target"], "trigger");
    assert_eq!(notices[0]["tap"]["to"]["id"], TRIGGER);

    pool.close().await;
    teardown_test_db(&db).await;
}
