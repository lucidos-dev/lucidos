//! The arming pass against a real database, a real bus and real child
//! processes. Each test drives [`arm_wait_for_running_tasks`], which the
//! engine wraps, then asks the dispatcher's own matcher whether the task's
//! completion would wake the thread.

use super::stop_tests::Stores;
use super::*;
use crate::engine::event_wait::catch_up_from_watermark;
use crate::engine::tools::bash_background::{CompletionDelivery, TaskSpec};
use crate::engine::tools::bash_background_recovery::completion_event;
use crate::test_support::seed_thread_event;
use std::path::Path;

/// A long-running task owned by `thread_id`, delivered as `delivery` says.
async fn task(s: &Stores, thread_id: Uuid, delivery: CompletionDelivery) -> String {
    run(s, thread_id, delivery, "sleep 30").await
}

/// `command` as a task owned by `thread_id`, delivered as `delivery` says.
async fn run(s: &Stores, thread_id: Uuid, delivery: CompletionDelivery, command: &str) -> String {
    let spec = TaskSpec {
        timeout_secs: 60,
        description: Some("the harden gates"),
        delivery,
        inherited: crate::core::InheritedSecrets::Keep,
    };
    let (task_id, _finished) = s
        .registry
        .spawn_task(
            &crate::runtime::agent_sandbox::Sandbox::unsandboxed(),
            command,
            Path::new("/tmp"),
            &[],
            Some(thread_id),
            spec,
        )
        .await
        .expect("spawn");
    task_id
}

/// Wait out the task, then persist its completion as its watcher would.
async fn finish_and_record(s: &Stores, task_id: &str, owner: Uuid) {
    assert!(
        s.registry
            .wait_for_finish(task_id, std::time::Duration::from_secs(15))
            .await,
        "task {task_id} never finished"
    );
    let record = s.registry.completion_record(task_id).await.expect("record");
    seed_thread_event(&s.bus, owner, completion_event(task_id.to_string(), record)).await;
}

async fn arm(s: &Stores, thread_id: Uuid, turn: OwnerTurn) -> Armed {
    arm_wait_for_running_tasks(&s.registry, &s.bus, &s.live, &s.pool, thread_id, turn).await
}

/// THE regression. An agent's turn ended with its polled gates still running,
/// and their completion re-opened nothing. A clean turn end now arms a wait
/// over them, and the completion wakes the thread.
#[tokio::test]
async fn an_idle_owners_polled_task_wakes_it_when_it_finishes() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let gates = task(&s, thread, CompletionDelivery::Poll).await;

    let armed = arm(&s, thread, OwnerTurn::Ended).await;
    let wait = armed
        .wait
        .expect("a clean turn end arms for the polled task");
    assert_eq!(armed.covered, vec![gates.clone()]);
    assert_eq!(wait.reason, "the harden gates to finish");
    assert!(is_engine_armed(&wait));

    s.registry.kill_for_thread(thread).await;
    let (event_type, payload) = s.completion(&gates, thread).await;
    assert_eq!(s.woken_by(&event_type, &payload).await, vec![wait.wait_id]);

    s.teardown().await;
}

/// A user Stop ends the turn without the turn-end arming, and the spawn route
/// left the polled task to the agent. So its completion wakes nothing, and
/// the Stop is not overridden.
#[tokio::test]
async fn a_polled_task_after_a_user_stop_wakes_nothing() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let gates = task(&s, thread, CompletionDelivery::Poll).await;

    let at_spawn = arm(&s, thread, OwnerTurn::Running).await;
    assert!(at_spawn.wait.is_none(), "the route never arms for --poll");
    assert!(at_spawn.covered.is_empty());

    s.registry.kill_for_thread(thread).await;
    let (event_type, payload) = s.completion(&gates, thread).await;
    assert!(
        s.woken_by(&event_type, &payload).await.is_empty(),
        "nothing armed, so nothing may wake the stopped thread"
    );

    s.teardown().await;
}

/// The next clean idle re-arms over what still runs, and never twice over a
/// task a live wait already covers.
#[tokio::test]
async fn a_second_turn_end_does_not_arm_twice_for_one_task() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let gates = task(&s, thread, CompletionDelivery::Poll).await;

    let first = arm(&s, thread, OwnerTurn::Ended).await;
    let wait = first.wait.expect("armed");
    let second = arm(&s, thread, OwnerTurn::Ended).await;
    assert!(second.wait.is_none());
    assert_eq!(second.covered, vec![gates.clone()]);
    assert_eq!(s.live.for_thread(thread).await.len(), 1);

    s.registry.kill_for_thread(thread).await;
    let (event_type, payload) = s.completion(&gates, thread).await;
    assert_eq!(s.woken_by(&event_type, &payload).await, vec![wait.wait_id]);

    s.teardown().await;
}

/// The gap the Codex review found. The polled gates finished mid-turn and
/// the agent never read them, so nothing was running at the clean turn end.
/// The turn end now delivers the result, and only once.
#[tokio::test]
async fn an_unread_result_reopens_its_idle_owner_once() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let gates = run(&s, thread, CompletionDelivery::Poll, "true").await;
    finish_and_record(&s, &gates, thread).await;

    let armed = arm(&s, thread, OwnerTurn::Ended).await;
    let wait = armed
        .wait
        .expect("a clean turn end delivers the unread result");
    assert!(armed.covered.is_empty(), "nothing was running to cover");
    let (_, event_type, payload, _) = catch_up_from_watermark(&s.pool, &wait)
        .await
        .expect("scan")
        .expect("the catch-up scan finds the completion below the horizon");
    assert_eq!(event_type, "BackgroundBashCompleted");
    assert_eq!(payload["task_id"], gates.as_str());

    let again = arm(&s, thread, OwnerTurn::Ended).await;
    assert!(
        again.wait.is_none(),
        "the live engine wait already delivers the result"
    );

    s.teardown().await;
}

/// The engine wait delivers its earliest match, which `mark_result_read`
/// stands in for here, as the delivery does.
async fn deliver(s: &Stores, wait: &LiveWait, thread: Uuid, task_id: &str) {
    s.live.take(wait.wait_id).await.expect("the wait is live");
    s.registry.mark_result_read(thread, task_id);
}

/// One wait delivers one completion. Two unread results take two clean
/// turn ends, and neither is lost.
#[tokio::test]
async fn each_unread_result_reopens_its_owner() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let gates = run(&s, thread, CompletionDelivery::Poll, "true").await;
    finish_and_record(&s, &gates, thread).await;
    let review = run(&s, thread, CompletionDelivery::Poll, "true").await;
    finish_and_record(&s, &review, thread).await;

    let first = arm(&s, thread, OwnerTurn::Ended).await.wait.expect("armed");
    deliver(&s, &first, thread, &gates).await;

    let second = arm(&s, thread, OwnerTurn::Ended)
        .await
        .wait
        .expect("the second result is still unread");
    let (_, _, payload, _) = catch_up_from_watermark(&s.pool, &second)
        .await
        .expect("scan")
        .expect("the catch-up scan finds the second completion");
    assert_eq!(payload["task_id"], review.as_str());

    s.teardown().await;
}

/// A wait over two running tasks fires on the first to finish. The second
/// then finishes unread, and the next clean turn end delivers it.
#[tokio::test]
async fn a_sibling_of_a_delivered_task_finishes_unread() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let gates = run(&s, thread, CompletionDelivery::Poll, "sleep 1").await;
    let review = run(&s, thread, CompletionDelivery::Poll, "sleep 1").await;

    let wait = arm(&s, thread, OwnerTurn::Ended).await.wait.expect("armed");
    finish_and_record(&s, &gates, thread).await;
    deliver(&s, &wait, thread, &gates).await;
    finish_and_record(&s, &review, thread).await;

    let ids: Vec<String> = s
        .registry
        .unread_results_for_thread(thread)
        .into_iter()
        .map(|r| r.task_id)
        .collect();
    assert_eq!(ids, vec![review.clone()]);
    assert!(arm(&s, thread, OwnerTurn::Ended).await.wait.is_some());

    s.teardown().await;
}

/// The spawn route never delivers an unread result: the owner's turn runs.
#[tokio::test]
async fn an_unread_result_is_left_to_its_owner_mid_turn() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let gates = run(&s, thread, CompletionDelivery::Poll, "true").await;
    finish_and_record(&s, &gates, thread).await;

    assert!(arm(&s, thread, OwnerTurn::Running).await.wait.is_none());
    assert_eq!(s.registry.unread_results_for_thread(thread).len(), 1);

    s.teardown().await;
}

/// A polled task the turn end armed for finishes into that wait. While the
/// wait is live, a turn ending before it delivers arms nothing more.
#[tokio::test]
async fn a_task_the_turn_end_armed_for_is_delivered_once() {
    let s = Stores::new().await;
    let thread = s.thread().await;
    let gates = run(&s, thread, CompletionDelivery::Poll, "sleep 1").await;

    let wait = arm(&s, thread, OwnerTurn::Ended).await.wait.expect("armed");
    finish_and_record(&s, &gates, thread).await;
    assert!(arm(&s, thread, OwnerTurn::Ended).await.wait.is_none());

    deliver(&s, &wait, thread, &gates).await;
    assert!(s.registry.unread_results_for_thread(thread).is_empty());
    assert!(arm(&s, thread, OwnerTurn::Ended).await.wait.is_none());

    s.teardown().await;
}
