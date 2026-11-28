use super::*;
use serde_json::json;

fn sub(event_type: &str, condition: Option<serde_json::Value>) -> EventSubscription {
    EventSubscription {
        event_type: event_type.to_string(),
        condition,
    }
}

fn owner() -> Uuid {
    Uuid::from_u128(0x0BACC0DE)
}

fn handle(task_id: &str, deadline: DateTime<Utc>) -> RunningTaskHandle {
    RunningTaskHandle {
        task_id: task_id.to_string(),
        description: None,
        command: "cargo build".to_string(),
        watchdog_deadline: deadline,
        queued_for_build_slot: false,
        stop_requested: false,
        polled: false,
    }
}

#[test]
fn a_wait_the_model_armed_for_this_task_covers_it() {
    let on = vec![sub(
        "BackgroundBashCompleted",
        Some(json!({"task_id": "abc"})),
    )];
    assert!(wait_covers_task(&on, "abc", Some(owner())));
}

#[test]
fn a_wait_for_a_different_task_does_not_cover_this_one() {
    let on = vec![sub(
        "BackgroundBashCompleted",
        Some(json!({"task_id": "other"})),
    )];
    assert!(!wait_covers_task(&on, "abc", Some(owner())));
}

#[test]
fn an_unconditioned_subscription_covers_every_task() {
    let on = vec![sub("BackgroundBashCompleted", None)];
    assert!(wait_covers_task(&on, "abc", Some(owner())));
    assert!(wait_covers_task(&on, "anything-else", Some(owner())));
}

#[test]
fn a_wait_on_another_event_type_covers_nothing() {
    let on = vec![
        sub("ChangeProposed", None),
        sub("CodingAgentIdled", Some(json!({"task_id": "abc"}))),
    ];
    assert!(!wait_covers_task(&on, "abc", Some(owner())));
}

#[test]
fn one_matching_entry_among_several_is_coverage() {
    let on = vec![
        sub("ChangeProposed", None),
        sub("BackgroundBashCompleted", Some(json!({"task_id": "abc"}))),
    ];
    assert!(wait_covers_task(&on, "abc", Some(owner())));
}

#[test]
fn the_wait_outlives_the_task_it_watches() {
    let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid");
    let task_secs = 3600;
    let tasks = [handle("a", now + Duration::seconds(task_secs))];
    let refs: Vec<&RunningTaskHandle> = tasks.iter().collect();

    let timeout = timeout_for(&refs, now);

    assert!(
        timeout > task_secs,
        "wait of {timeout}s must outlive a task with {task_secs}s left"
    );
    assert_eq!(timeout, task_secs + DEADLINE_MARGIN.num_seconds());
}

#[test]
fn several_tasks_size_the_wait_on_the_latest_deadline() {
    let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid");
    let tasks = [
        handle("short", now + Duration::seconds(60)),
        handle("long", now + Duration::seconds(7200)),
        handle("middling", now + Duration::seconds(900)),
    ];
    let refs: Vec<&RunningTaskHandle> = tasks.iter().collect();

    assert_eq!(
        timeout_for(&refs, now),
        7200 + DEADLINE_MARGIN.num_seconds()
    );
}

#[test]
fn a_deadline_already_past_yields_the_floor_not_a_negative() {
    let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid");
    let tasks = [handle("overdue", now - Duration::hours(24))];
    let refs: Vec<&RunningTaskHandle> = tasks.iter().collect();

    assert_eq!(timeout_for(&refs, now), 1);
}

#[test]
fn the_ordinary_ceiling_still_applies() {
    let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid");
    let tasks = [handle("marathon", now + Duration::days(7))];
    let refs: Vec<&RunningTaskHandle> = tasks.iter().collect();

    assert_eq!(
        timeout_for(&refs, now),
        super::super::register::MAX_TIMEOUT_SECS
    );
}

fn label(t: &RunningTaskHandle) -> String {
    task_label(t.description.as_deref(), &t.command)
}

fn named(description: Option<&str>, command: &str) -> RunningTaskHandle {
    RunningTaskHandle {
        description: description.map(String::from),
        command: command.to_string(),
        ..task("t")
    }
}

#[test]
fn one_described_task_is_named_in_the_agents_words() {
    let t = named(Some("the five-project e2e sweep"), "./scripts/sweep.sh");
    assert_eq!(
        armed_reason(&[label(&t)]),
        "the five-project e2e sweep to finish"
    );
}

#[test]
fn an_undescribed_task_is_named_by_its_command() {
    let t = named(None, "cargo test --lib");
    assert_eq!(
        armed_reason(&[label(&t)]),
        "the background command \"cargo test --lib\" to finish"
    );
}

#[test]
fn several_tasks_are_all_named_and_the_first_to_finish_is_what_counts() {
    let a = named(Some("the build"), "make");
    let b = named(None, "npm test");
    assert_eq!(
        armed_reason(&[label(&a), label(&b)]),
        "the first of 2 background jobs to finish: the build; \
         the background command \"npm test\""
    );
}

#[test]
fn tasks_past_the_cap_are_counted() {
    let tasks: Vec<RunningTaskHandle> = (0..5)
        .map(|i| named(Some(&format!("job {i}")), "x"))
        .collect();
    let labels: Vec<String> = tasks.iter().map(label).collect();
    assert_eq!(
        armed_reason(&labels),
        "the first of 5 background jobs to finish: job 0; job 1; job 2; 2 more"
    );
}

#[test]
fn a_long_multi_line_command_becomes_one_bounded_line() {
    let command = format!("set -e\ncd /tmp\n{}", "echo lots; ".repeat(50));
    let t = named(None, &command);
    let reason = armed_reason(&[label(&t)]);

    assert!(!reason.contains('\n'), "{reason}");
    assert!(reason.contains("\"set -e cd /tmp echo lots;"), "{reason}");
    assert!(
        reason.contains('…'),
        "a cut label says it was cut: {reason}"
    );
    assert!(one_short_line(&command).chars().count() <= LABEL_MAX_CHARS);
}

#[test]
fn the_synthetic_tool_use_id_is_namespaced() {
    assert!(
        ENGINE_TOOL_USE_PREFIX.starts_with("engine:"),
        "an engine-armed wait must be distinguishable from a model-armed one"
    );
}

fn wait_over(on: Vec<EventSubscription>) -> super::super::LiveWait {
    let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid");
    super::super::LiveWait {
        background_timing: None,
        wait_id: uuid::Uuid::new_v4(),
        thread_id: uuid::Uuid::new_v4(),
        tool_use_id: "toolu_test".to_string(),
        on,
        reason: "test".to_string(),
        armed_at: now,
        expires_at: now + Duration::hours(1),
        watermark: 0,
    }
}

fn task(id: &str) -> RunningTaskHandle {
    handle(
        id,
        DateTime::from_timestamp(1_800_003_600, 0).expect("valid"),
    )
}

#[test]
fn an_unwatched_task_is_armed() {
    let running = [task("build")];
    match plan_wait(&running, &[], &[], Some(0), owner(), OwnerTurn::Ended) {
        ArmingPlan::Arm(Uncovered { running: tasks, .. }) => assert_eq!(tasks.len(), 1),
        other => panic!("expected Arm, got {other:?}"),
    }
}

#[test]
fn a_task_the_model_is_already_watching_is_not_armed_again() {
    let running = [task("build")];
    let live = [wait_over(vec![sub(
        "BackgroundBashCompleted",
        Some(json!({"task_id": "build"})),
    )])];
    assert_eq!(
        plan_wait(&running, &[], &live, Some(0), owner(), OwnerTurn::Ended),
        ArmingPlan::NothingUncovered
    );
}

#[test]
fn only_the_uncovered_tasks_are_armed() {
    let running = [task("watched"), task("unwatched")];
    let live = [wait_over(vec![sub(
        "BackgroundBashCompleted",
        Some(json!({"task_id": "watched"})),
    )])];
    match plan_wait(&running, &[], &live, Some(0), owner(), OwnerTurn::Ended) {
        ArmingPlan::Arm(Uncovered { running: tasks, .. }) => {
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].task_id, "unwatched");
        }
        other => panic!("expected Arm, got {other:?}"),
    }
}

#[test]
fn several_uncovered_tasks_go_into_one_wait() {
    let running = [task("a"), task("b"), task("c")];
    match plan_wait(&running, &[], &[], Some(0), owner(), OwnerTurn::Ended) {
        ArmingPlan::Arm(Uncovered { running: tasks, .. }) => assert_eq!(tasks.len(), 3),
        other => panic!("expected Arm, got {other:?}"),
    }
}

#[test]
fn the_live_wait_cap_refuses_and_says_why() {
    let running = [task("build")];
    let live: Vec<super::super::LiveWait> = (0..MAX_LIVE_WAITS_PER_THREAD)
        .map(|_| wait_over(vec![sub("ChangeProposed", None)]))
        .collect();
    match plan_wait(&running, &[], &live, Some(0), owner(), OwnerTurn::Ended) {
        ArmingPlan::Refused(why) => assert!(why.contains("live subscriptions"), "{why}"),
        other => panic!("expected Refused, got {other:?}"),
    }
}

#[test]
fn the_recent_subscription_cap_refuses_at_the_same_limit_the_model_gets() {
    let running = [task("build")];
    assert!(matches!(
        plan_wait(
            &running,
            &[],
            &[],
            Some(super::super::MAX_RECENT_SUBSCRIPTIONS),
            owner(),
            OwnerTurn::Ended,
        ),
        ArmingPlan::Refused(_)
    ));
    assert!(matches!(
        plan_wait(
            &running,
            &[],
            &[],
            Some(super::super::MAX_RECENT_SUBSCRIPTIONS - 1),
            owner(),
            OwnerTurn::Ended,
        ),
        ArmingPlan::Arm(_)
    ));
}

#[test]
fn an_unreadable_subscription_count_refuses_rather_than_assuming_zero() {
    let running = [task("build")];
    match plan_wait(&running, &[], &[], None, owner(), OwnerTurn::Ended) {
        ArmingPlan::Refused(why) => assert!(why.contains("could not be read"), "{why}"),
        other => panic!("an unknown count must refuse, got {other:?}"),
    }
}

#[test]
fn a_task_being_stopped_is_never_armed_for() {
    let mut stopping = task("lint");
    stopping.stop_requested = true;
    let running = [stopping, task("e2e")];
    match plan_wait(&running, &[], &[], Some(0), owner(), OwnerTurn::Ended) {
        ArmingPlan::Arm(Uncovered { running: tasks, .. }) => {
            assert_eq!(
                tasks.iter().map(|h| h.task_id.as_str()).collect::<Vec<_>>(),
                ["e2e"]
            )
        }
        other => panic!("expected Arm, got {other:?}"),
    }
    assert_eq!(
        plan_wait(&running[..1], &[], &[], Some(0), owner(), OwnerTurn::Ended),
        ArmingPlan::NothingUncovered
    );
}

#[test]
fn a_polled_task_is_not_armed_for_while_its_owners_turn_runs() {
    let mut polled = task("suite");
    polled.polled = true;
    let running = [polled, task("e2e")];
    match plan_wait(&running, &[], &[], Some(0), owner(), OwnerTurn::Running) {
        ArmingPlan::Arm(Uncovered { running: tasks, .. }) => {
            assert_eq!(
                tasks.iter().map(|h| h.task_id.as_str()).collect::<Vec<_>>(),
                ["e2e"]
            )
        }
        other => panic!("expected Arm, got {other:?}"),
    }
    assert_eq!(
        plan_wait(
            &running[..1],
            &[],
            &[],
            Some(0),
            owner(),
            OwnerTurn::Running
        ),
        ArmingPlan::NothingUncovered
    );
}

#[test]
fn a_polled_task_is_armed_for_once_its_owners_turn_ends() {
    let mut polled = task("harden gates");
    polled.polled = true;
    let running = [polled, task("e2e")];
    match plan_wait(&running, &[], &[], Some(0), owner(), OwnerTurn::Ended) {
        ArmingPlan::Arm(Uncovered { running: tasks, .. }) => {
            assert_eq!(
                tasks.iter().map(|h| h.task_id.as_str()).collect::<Vec<_>>(),
                ["harden gates", "e2e"]
            )
        }
        other => panic!("expected Arm, got {other:?}"),
    }
}

#[test]
fn a_polled_task_being_stopped_is_not_armed_for_at_turn_end() {
    let mut stopping = task("suite");
    stopping.polled = true;
    stopping.stop_requested = true;
    assert_eq!(
        plan_wait(&[stopping], &[], &[], Some(0), owner(), OwnerTurn::Ended),
        ArmingPlan::NothingUncovered
    );
}

fn unread_result(id: &str) -> UnreadResult {
    UnreadResult {
        task_id: id.to_string(),
        description: Some("the harden gates".to_string()),
        command: "make lint".to_string(),
    }
}

#[test]
fn an_unread_result_is_armed_for_once_its_owners_turn_ends() {
    let unread = [unread_result("gates")];
    match plan_wait(&[], &unread, &[], Some(0), owner(), OwnerTurn::Ended) {
        ArmingPlan::Arm(uncovered) => {
            assert!(uncovered.running.is_empty());
            assert_eq!(uncovered.task_ids(), ["gates"]);
            assert_eq!(
                armed_reason(&uncovered.labels()),
                "the harden gates to finish"
            );
        }
        other => panic!("expected Arm, got {other:?}"),
    }
}

#[test]
fn an_unread_result_is_not_armed_for_while_its_owners_turn_runs() {
    let unread = [unread_result("gates")];
    assert_eq!(
        plan_wait(&[], &unread, &[], Some(0), owner(), OwnerTurn::Running),
        ArmingPlan::NothingUncovered
    );
}

#[test]
fn only_a_live_engine_wait_covers_an_unread_result() {
    let unread = [unread_result("gates")];
    let engine = [wait_armed_by(
        &engine_tool_use_id(),
        vec![completed("gates")],
    )];
    assert_eq!(
        plan_wait(&[], &unread, &engine, Some(0), owner(), OwnerTurn::Ended),
        ArmingPlan::NothingUncovered
    );
    let model = [wait_armed_by("toolu_x", vec![completed("gates")])];
    assert!(matches!(
        plan_wait(&[], &unread, &model, Some(0), owner(), OwnerTurn::Ended),
        ArmingPlan::Arm(_)
    ));
}

#[test]
fn an_unread_result_is_refused_at_the_recent_subscription_cap() {
    let unread = [unread_result("gates")];
    assert!(matches!(
        plan_wait(
            &[],
            &unread,
            &[],
            Some(super::super::MAX_RECENT_SUBSCRIPTIONS),
            owner(),
            OwnerTurn::Ended,
        ),
        ArmingPlan::Refused(_)
    ));
}

fn wait_armed_by(tool_use_id: &str, on: Vec<EventSubscription>) -> super::super::LiveWait {
    super::super::LiveWait {
        tool_use_id: tool_use_id.to_string(),
        ..wait_over(on)
    }
}

fn completed(task_id: &str) -> EventSubscription {
    sub(
        "BackgroundBashCompleted",
        Some(json!({ "task_id": task_id })),
    )
}

#[test]
fn only_the_engines_own_prefix_counts_as_engine_armed() {
    assert!(is_engine_armed(&wait_armed_by(
        &engine_tool_use_id(),
        vec![]
    )));
    assert!(!is_engine_armed(&wait_armed_by(
        "engine:bg-task-waiting",
        vec![]
    )));
    assert!(!is_engine_armed(&wait_armed_by("toolu_x", vec![])));
}

#[test]
fn a_wait_over_only_the_stopped_task_ends() {
    let wait = wait_armed_by(&engine_tool_use_id(), vec![completed("lint")]);
    assert_eq!(
        plan_stand_down(&wait, "lint", Some(owner())),
        Some(StandDown::End)
    );
}

#[test]
fn a_wait_that_never_fires_on_the_task_is_left_alone() {
    let wait = wait_armed_by(&engine_tool_use_id(), vec![completed("e2e")]);
    assert_eq!(plan_stand_down(&wait, "lint", Some(owner())), None);
    let other = wait_armed_by("toolu_x", vec![sub("ChangeProposed", None)]);
    assert_eq!(plan_stand_down(&other, "lint", Some(owner())), None);
}

#[test]
fn the_engines_wait_over_more_tasks_narrows_to_the_rest() {
    let wait = wait_armed_by(
        &engine_tool_use_id(),
        vec![completed("lint"), completed("e2e")],
    );
    assert_eq!(
        plan_stand_down(&wait, "lint", Some(owner())),
        Some(StandDown::Narrow(vec![completed("e2e")]))
    );
}

#[test]
fn a_model_wait_watching_more_ends_whole() {
    let wait = wait_armed_by(
        "toolu_x",
        vec![completed("lint"), sub("ChangeProposed", None)],
    );
    assert_eq!(
        plan_stand_down(&wait, "lint", Some(owner())),
        Some(StandDown::EndWithOthers)
    );
    let only = wait_armed_by("toolu_x", vec![completed("lint")]);
    assert_eq!(
        plan_stand_down(&only, "lint", Some(owner())),
        Some(StandDown::End)
    );
}

#[test]
fn the_owner_decides_whether_a_thread_conditioned_entry_fires() {
    let elsewhere = Uuid::from_u128(0xE15E);
    let wait = wait_armed_by(
        "toolu_x",
        vec![sub(
            "BackgroundBashCompleted",
            Some(json!({ "task_id": "lint", "thread_id": elsewhere.to_string() })),
        )],
    );
    assert_eq!(plan_stand_down(&wait, "lint", Some(owner())), None);
    assert_eq!(
        plan_stand_down(&wait, "lint", Some(elsewhere)),
        Some(StandDown::End)
    );
}

#[test]
fn a_condition_on_how_the_task_ended_is_judged_against_a_stop() {
    let killed = wait_armed_by(
        "toolu_x",
        vec![sub(
            "BackgroundBashCompleted",
            Some(json!({ "task_id": "lint", "killed": true })),
        )],
    );
    assert_eq!(
        plan_stand_down(&killed, "lint", Some(owner())),
        Some(StandDown::End)
    );
    let natural = wait_armed_by(
        "toolu_x",
        vec![sub(
            "BackgroundBashCompleted",
            Some(json!({ "task_id": "lint", "killed": false })),
        )],
    );
    assert_eq!(plan_stand_down(&natural, "lint", Some(owner())), None);
}
