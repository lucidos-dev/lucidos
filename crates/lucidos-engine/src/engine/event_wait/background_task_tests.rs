//! The pure decisions behind the engine-armed background-task wait: whether to
//! arm at all, over which tasks, and for how long.
//!
//! Both are the parts that can be wrong silently. A coverage test that is too
//! loose wakes the thread twice for one completion; one that is too strict
//! never arms at all and the stall comes straight back. A timeout shorter than
//! the task is a subscription that expires before the thing it watches.
//!
//! `plan_wait` exists as a pure function precisely so all of that is reachable
//! without standing up an engine and a database. What is left in the engine
//! method is three reads, this call, and the arming itself.

use super::*;
use serde_json::json;

fn sub(event_type: &str, condition: Option<serde_json::Value>) -> EventSubscription {
    EventSubscription {
        event_type: event_type.to_string(),
        condition,
    }
}

fn handle(task_id: &str, deadline: DateTime<Utc>) -> RunningTaskHandle {
    RunningTaskHandle {
        task_id: task_id.to_string(),
        watchdog_deadline: deadline,
    }
}

// ── coverage ─────────────────────────────────────────────────────────

/// The case that matters: the model armed its own wait for exactly this task,
/// so the engine must not arm a second one. Two waits over one completion is
/// two wakes, and the second finds the work already reported.
#[test]
fn a_wait_the_model_armed_for_this_task_covers_it() {
    let on = vec![sub(
        "BackgroundBashCompleted",
        Some(json!({"task_id": "abc"})),
    )];
    assert!(wait_covers_task(&on, "abc"));
}

/// The mirror case, and the reason coverage is not just "does a
/// `BackgroundBashCompleted` entry exist": a wait for a DIFFERENT task will
/// never fire for this one, so this task is still unwatched.
#[test]
fn a_wait_for_a_different_task_does_not_cover_this_one() {
    let on = vec![sub(
        "BackgroundBashCompleted",
        Some(json!({"task_id": "other"})),
    )];
    assert!(!wait_covers_task(&on, "abc"));
}

/// An unconditioned subscription wakes on the first background task to finish
/// anywhere, including this one, so it genuinely covers it. Arming beside it
/// would double the wake.
#[test]
fn an_unconditioned_subscription_covers_every_task() {
    let on = vec![sub("BackgroundBashCompleted", None)];
    assert!(wait_covers_task(&on, "abc"));
    assert!(wait_covers_task(&on, "anything-else"));
}

/// A thread waiting on something else entirely is not watching its background
/// work, however many live subscriptions it holds.
#[test]
fn a_wait_on_another_event_type_covers_nothing() {
    let on = vec![
        sub("ChangeProposed", None),
        sub("CodingAgentIdled", Some(json!({"task_id": "abc"}))),
    ];
    assert!(!wait_covers_task(&on, "abc"));
}

/// The `on:` list is an OR, so one matching entry among several is coverage.
#[test]
fn one_matching_entry_among_several_is_coverage() {
    let on = vec![
        sub("ChangeProposed", None),
        sub("BackgroundBashCompleted", Some(json!({"task_id": "abc"}))),
    ];
    assert!(wait_covers_task(&on, "abc"));
}

// ── timeout ──────────────────────────────────────────────────────────

/// The invariant the deadline exists for: the wait outlives the task. A task
/// killed by its watchdog at T emits its completion at or after T, so a wait
/// expiring at T could lose that race.
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

/// One wait covers several tasks, so it has to last past the LAST of them.
/// Sizing on the first would strand every longer task in the same list.
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

/// A watchdog that is late (a child ignoring SIGTERM, a saturated host) leaves
/// a deadline in the past. The answer is a wait that gives up almost at once,
/// never a negative timeout, which `Duration::seconds` would turn into an
/// `expires_at` BEFORE `armed_at` and so a subscription that can never fire.
#[test]
fn a_deadline_already_past_yields_the_floor_not_a_negative() {
    let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid");
    let tasks = [handle("overdue", now - Duration::hours(24))];
    let refs: Vec<&RunningTaskHandle> = tasks.iter().collect();

    assert_eq!(timeout_for(&refs, now), 1);
}

/// The ceiling is the same one `await_event` enforces. A task spawned with a
/// multi-day watchdog must not produce a wait that outlives the ordinary
/// maximum, which exists because a wait outliving every reason for it is
/// indistinguishable from a stalled thread.
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

// ── the synthetic id ─────────────────────────────────────────────────

/// The id is namespaced so an engine-armed wait is recognisable in the event
/// log and cannot collide with a provider-issued `tool_use_id`.
#[test]
fn the_synthetic_tool_use_id_is_namespaced() {
    assert!(
        ENGINE_TOOL_USE_PREFIX.starts_with("engine:"),
        "an engine-armed wait must be distinguishable from a model-armed one"
    );
}

// ── the arming decision ──────────────────────────────────────────────

fn wait_over(on: Vec<EventSubscription>) -> super::super::LiveWait {
    let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid");
    super::super::LiveWait {
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

/// The ordinary case: a turn ends with a build running and nothing watching,
/// which is the five-hour stall this whole module exists to prevent.
#[test]
fn an_unwatched_task_is_armed() {
    let running = [task("build")];
    match plan_wait(&running, &[], Some(0)) {
        ArmingPlan::Arm(tasks) => assert_eq!(tasks.len(), 1),
        other => panic!("expected Arm, got {other:?}"),
    }
}

/// The model armed its own wait, so the engine must stand down. Two waits over
/// one completion is two wakes.
#[test]
fn a_task_the_model_is_already_watching_is_not_armed_again() {
    let running = [task("build")];
    let live = [wait_over(vec![sub(
        "BackgroundBashCompleted",
        Some(json!({"task_id": "build"})),
    )])];
    assert_eq!(
        plan_wait(&running, &live, Some(0)),
        ArmingPlan::NothingUncovered
    );
}

/// Partial coverage still arms, over the remainder only. Arming over the
/// covered one too would double its wake.
#[test]
fn only_the_uncovered_tasks_are_armed() {
    let running = [task("watched"), task("unwatched")];
    let live = [wait_over(vec![sub(
        "BackgroundBashCompleted",
        Some(json!({"task_id": "watched"})),
    )])];
    match plan_wait(&running, &live, Some(0)) {
        ArmingPlan::Arm(tasks) => {
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].task_id, "unwatched");
        }
        other => panic!("expected Arm, got {other:?}"),
    }
}

/// One wait covers every uncovered task rather than one wait each, so three
/// builds spend one live-wait slot instead of three.
#[test]
fn several_uncovered_tasks_go_into_one_wait() {
    let running = [task("a"), task("b"), task("c")];
    match plan_wait(&running, &[], Some(0)) {
        ArmingPlan::Arm(tasks) => assert_eq!(tasks.len(), 3),
        other => panic!("expected Arm, got {other:?}"),
    }
}

/// The live-wait cap. At the limit the thread goes quiet with work running,
/// which is a real regression, so the plan says why rather than returning a
/// bare no.
#[test]
fn the_live_wait_cap_refuses_and_says_why() {
    let running = [task("build")];
    let live: Vec<super::super::LiveWait> = (0..MAX_LIVE_WAITS_PER_THREAD)
        .map(|_| wait_over(vec![sub("ChangeProposed", None)]))
        .collect();
    match plan_wait(&running, &live, Some(0)) {
        ArmingPlan::Refused(why) => assert!(why.contains("live subscriptions"), "{why}"),
        other => panic!("expected Refused, got {other:?}"),
    }
}

/// The consecutive cap is what bounds the loop this mechanism could otherwise
/// create: a turn woken by an engine-armed wait spawns another task and ends
/// again, forever. Counting engine-armed waits stops it at the same ten the
/// model gets.
#[test]
fn the_consecutive_cap_refuses_at_the_same_limit_the_model_gets() {
    let running = [task("build")];
    assert!(matches!(
        plan_wait(
            &running,
            &[],
            Some(super::super::MAX_CONSECUTIVE_SUBSCRIPTIONS)
        ),
        ArmingPlan::Refused(_)
    ));
    assert!(matches!(
        plan_wait(
            &running,
            &[],
            Some(super::super::MAX_CONSECUTIVE_SUBSCRIPTIONS - 1)
        ),
        ArmingPlan::Arm(_)
    ));
}

/// A cap that cannot be evaluated must not silently become no cap: an
/// unreadable event store is exactly when a runaway loop does the most damage.
/// `None` is UNKNOWN, never zero.
#[test]
fn an_unreadable_subscription_count_refuses_rather_than_assuming_zero() {
    let running = [task("build")];
    match plan_wait(&running, &[], None) {
        ArmingPlan::Refused(why) => assert!(why.contains("could not be read"), "{why}"),
        other => panic!("an unknown count must refuse, got {other:?}"),
    }
}
