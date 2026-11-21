//! A child a restart cut, and the parent that must hear about it truthfully.
//! Plan: `docs/plans/2026-10-03-crash-cut-child-reports-truthfully.md`.

use super::super::*;
use super::*;
use crate::engine::thread_events::AbortCause;

/// Give `child_id` a finished turn whose text a later card must NOT reuse.
async fn emit_prior_response(bus: &EventBus, child_id: Uuid, text: &str) {
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::ResponseGenerated {
            text: text.into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

async fn emit_abort(bus: &EventBus, thread_id: Uuid, cause: AbortCause, actor: MessageOrigin) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause,
        },
        meta: EventMeta {
            actor: Some(actor),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// The boot race: recovery's crash idle on a child must not wake the parent
/// while the recovery sweeps still run. The boot refire then re-reads the
/// same card, and the two still come to one wake.
#[tokio::test]
async fn a_wake_recovery_causes_waits_for_the_release_and_runs_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    bus.hold_parent_wakes();
    emit_cc_recovery_pair(&bus, child_id).await;

    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        0,
        "no parent turn may start while boot recovery runs"
    );

    bus.refire_unprocessed_child_completions().await;
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        0,
        "the refire is held too"
    );

    assert_eq!(bus.release_held_parent_wakes(), 1);
    assert_eq!(
        drain_callbacks(&mut callback_rx),
        1,
        "the recovery wake and the refire of its card are one turn"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A coding-agent child a crash cut has not finished. The card says
/// `interrupted`, and never passes the previous turn's text off as a result.
#[tokio::test]
async fn a_crash_cut_coding_agent_child_reports_interrupted() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;
    emit_prior_response(&bus, child_id, "I'm waiting for chunks 48 to 52.").await;

    emit_cc_recovery_pair(&bus, child_id).await;

    assert_eq!(
        count_completion_cards(&pool, parent_id).await,
        1,
        "the abort reports, and the pending-card guard swallows the idle after it"
    );
    let (status, summary) = newest_completion_card(&pool, parent_id).await;
    assert_eq!(status, "interrupted");
    assert!(
        !summary.contains("chunks 48"),
        "the summary must not reuse the previous turn's text: {summary}"
    );
    assert!(
        summary.contains("follow_up_child_thread"),
        "the summary tells the parent how to continue: {summary}"
    );
    assert_eq!(drain_callbacks(&mut callback_rx), 1);
    assert_active_children(&pool, parent_id, 0, "the cut child is not in flight").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A chat child a crash cut used to send nothing at all, and its parent
/// waited forever: nothing auto-resumes after a crash.
#[tokio::test]
async fn a_crash_cut_chat_child_reports_interrupted() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;

    emit_abort(
        &bus,
        child_id,
        AbortCause::RecoveryAfterRestart,
        MessageOrigin::system(),
    )
    .await;

    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    let (status, _) = newest_completion_card(&pool, parent_id).await;
    assert_eq!(status, "interrupted");
    assert_eq!(drain_callbacks(&mut callback_rx), 1);
    assert_active_children(&pool, parent_id, 0, "the cut child is not in flight").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A teardown nobody asked for (`stop.sh`, ctrl-c) resumes nothing either.
#[tokio::test]
async fn a_chat_child_cut_by_an_unrequested_shutdown_reports_interrupted() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;

    emit_abort(
        &bus,
        child_id,
        AbortCause::EngineShutdown,
        MessageOrigin::system(),
    )
    .await;

    let (status, _) = newest_completion_card(&pool, parent_id).await;
    assert_eq!(status, "interrupted");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The user's own switch promises a resume, so the child is still working
/// and the parent hears nothing until the resumed turn ends.
#[tokio::test]
async fn a_chat_child_paused_by_a_user_switch_sends_no_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;

    emit_abort(
        &bus,
        child_id,
        AbortCause::EngineShutdown,
        MessageOrigin::Device {
            device_id: "test-device".into(),
        },
    )
    .await;

    assert_eq!(count_completion_cards(&pool, parent_id).await, 0);
    assert_eq!(drain_callbacks(&mut callback_rx), 0);
    assert_active_children(&pool, parent_id, 1, "the paused child is still in flight").await;

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// After an interrupted card, a continued turn reports again on its own.
#[tokio::test]
async fn a_chat_child_continued_after_a_crash_reports_its_new_turn() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::Chat).await;
    emit_abort(
        &bus,
        child_id,
        AbortCause::RecoveryAfterRestart,
        MessageOrigin::system(),
    )
    .await;

    emit_thread_message(&bus, child_id, Some(parent_id), "continue").await;
    emit_prior_response(&bus, child_id, "done now").await;

    assert_eq!(count_completion_cards(&pool, parent_id).await, 2);
    let (status, summary) = newest_completion_card(&pool, parent_id).await;
    assert_eq!((status.as_str(), summary.as_str()), ("success", "done now"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The running settle aborts a coding-agent child whose worktree recovery
/// never picked up, and no recovery idle follows. The abort alone reports.
#[tokio::test]
async fn a_coding_agent_child_cut_with_no_recovery_idle_still_reports() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, mut callback_rx) = EventBus::new(pool.clone());
    let (parent_id, child_id) = spawn_parent_child(&bus, EventChannel::ClaudeCode).await;
    emit_cc_session_started(&bus, child_id).await;

    emit_abort(
        &bus,
        child_id,
        AbortCause::RecoveryAfterRestart,
        MessageOrigin::system(),
    )
    .await;

    assert_eq!(count_completion_cards(&pool, parent_id).await, 1);
    let (status, _) = newest_completion_card(&pool, parent_id).await;
    assert_eq!(status, "interrupted");
    assert_eq!(drain_callbacks(&mut callback_rx), 1);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
