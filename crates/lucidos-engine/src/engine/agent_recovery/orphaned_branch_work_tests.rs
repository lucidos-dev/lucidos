use super::*;
use crate::core::changes::ChangeStatus;
use crate::engine::event_bus::EmittedEvent;
use crate::engine::thread_events::EventWaitCancelCause;
use crate::test_support::{
    make_repo_and_worktree, setup_test_db, start_cc_session, teardown_test_db,
};
use tokio::sync::broadcast;

/// How many `ChangeProposed` the trigger matcher was handed since the last
/// call: each one fires every trigger subscribed to `ChangeProposed`.
fn change_proposed_dispatches(rx: &mut broadcast::Receiver<EmittedEvent>) -> usize {
    let mut count = 0;
    while let Ok(emitted) = rx.try_recv() {
        if crate::scheduler::trigger_dispatch(&emitted)
            .is_some_and(|d| d.event_type == "ChangeProposed")
        {
            count += 1;
        }
    }
    count
}

/// The upgrade burst: the boot pass records an archived thread's unproposed
/// branch work once. Every later pass, and every later archive, finds the
/// change row and stays silent, so a trigger never hears a re-sync.
#[tokio::test]
async fn a_second_pass_never_re_announces_set_aside_branch_work() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/archived-branch-work";
    let (_tmp, repo, wt) = make_repo_and_worktree(branch).await;
    std::fs::write(wt.join("a.txt"), "archived work").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "archived work"], &wt)
        .await
        .unwrap();

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, branch, None).await;
    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .expect("archive the thread");

    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };
    let mut rx = bus.subscribe();

    assert_eq!(scope.set_aside_archived_branch_work_on_startup().await, 1);
    assert_eq!(
        change_proposed_dispatches(&mut rx),
        1,
        "the first pass records the work once"
    );

    assert_eq!(scope.set_aside_archived_branch_work_on_startup().await, 0);
    assert!(!scope
        .set_aside_archived_branch_work(thread_id)
        .await
        .expect("archive half runs"));
    assert_eq!(
        change_proposed_dispatches(&mut rx),
        0,
        "a second boot pass or archive gives a trigger nothing"
    );

    let persisted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, 1, "one ChangeProposed in the timeline");
    let change = bus
        .changes_projection()
        .get_open_by_branch(branch)
        .await
        .unwrap()
        .expect("the Changes panel has the set-aside change");
    assert_eq!(change.status(), ChangeStatus::SetAside);
    assert_eq!(change.files, vec!["a.txt".to_string()]);

    pool.close().await;
    teardown_test_db(&db).await;
}

fn change_proposed(set_aside: bool) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        change_id: Uuid::new_v4().to_string(),
        description: Some("work".into()),
        requires_restart: false,
        files: vec!["a.txt".into()],
        origin: None,
        commit_sha: None,
        branch_name: "claude-code/any".into(),
        repo_root: "/repo".into(),
        hardened: false,
        incomplete: set_aside,
        set_aside,
        path: String::new(),
        diff: String::new(),
    }
}

/// The filter `triggers.md` gives a "changes to apply" trigger. A pending
/// proposal omits `set_aside`, so a bare `false` would match nothing.
#[test]
fn the_documented_changes_to_apply_filter_skips_only_set_aside_work() {
    use crate::core::event_subscription::{condition, matchable_thread_payload};
    let filter = serde_json::json!({ "set_aside": { "$ne": true } });
    let thread_id = Uuid::new_v4();
    let pending = matchable_thread_payload(&change_proposed(false), &EventMeta::NONE, thread_id);
    let parked = matchable_thread_payload(&change_proposed(true), &EventMeta::NONE, thread_id);
    assert!(condition::evaluate(Some(&filter), &pending));
    assert!(!condition::evaluate(Some(&filter), &parked));
    assert!(
        !condition::evaluate(Some(&serde_json::json!({ "set_aside": false })), &pending),
        "a bare false misses the pending proposal, which is why the doc says $ne"
    );
}

fn cc_meta() -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    }
}

fn wait_canceled(wait_id: Uuid, cause: EventWaitCancelCause) -> ThreadEvent {
    ThreadEvent::EventWaitCanceled {
        wait_id,
        cause,
        on: Vec::new(),
        reason: String::new(),
    }
}

/// A coding-agent thread whose last turn ended `terminal`, with one committed
/// file on its branch.
async fn thread_with_branch_work(
    bus: &EventBus,
    branch: &str,
    terminal: ThreadEvent,
) -> (tempfile::TempDir, PathBuf, Uuid) {
    let (tmp, repo, wt) = make_repo_and_worktree(branch).await;
    std::fs::write(wt.join("a.txt"), "held work").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "held work"], &wt).await.unwrap();
    let thread_id = Uuid::new_v4();
    start_cc_session(bus, thread_id, branch, None).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: terminal,
        meta: cc_meta(),
    })
    .await
    .expect("emit the turn's terminal");
    (tmp, repo, thread_id)
}

fn generated() -> ThreadEvent {
    ThreadEvent::ResponseGenerated {
        text: "Not done yet: waiting on the suite.".into(),
        images: Vec::new(),
        model: None,
        reasoning_effort: None,
    }
}

/// ADR 0395: the idle held its proposal while the wait was live. Once the
/// last wait is canceled, the work falls due as complete work, because the
/// turn ended cleanly. While any wait is left, nothing is due.
#[tokio::test]
async fn a_canceled_last_wait_releases_the_held_work() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/held-by-a-wait";
    let (_tmp, repo, thread_id) = thread_with_branch_work(&bus, branch, generated()).await;
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
    for wait_id in [first, second] {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: crate::test_support::event_wait_started(wait_id),
            meta: EventMeta::NONE,
        })
        .await
        .expect("arm a wait");
    }
    assert!(
        scope.work_a_canceled_wait_held(thread_id).await.is_none(),
        "a live wait still holds the work"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: wait_canceled(first, EventWaitCancelCause::UserStop),
        meta: EventMeta::NONE,
    })
    .await
    .expect("cancel one wait");
    assert!(
        scope.work_a_canceled_wait_held(thread_id).await.is_none(),
        "the second wait still holds the work"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: wait_canceled(second, EventWaitCancelCause::AgentStandDown),
        meta: EventMeta::NONE,
    })
    .await
    .expect("cancel the last wait");
    let (work, turn_end) = scope
        .work_a_canceled_wait_held(thread_id)
        .await
        .expect("the last cancel releases the work");
    assert_eq!(work.branch_name, branch);
    assert_eq!(work.files, vec!["a.txt".to_string()]);
    assert_eq!(
        turn_end,
        TurnEnd::Finished,
        "a clean turn's released work is proposed"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The release re-checks the thread rather than trusting the cancel: a running
/// turn's own idle proposes, an archive sets the work aside instead, and a
/// failed turn recovers through Continue.
#[tokio::test]
async fn held_work_is_released_only_for_an_idle_open_thread_whose_turn_proposes() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let (_tmp_a, repo_a, running) =
        thread_with_branch_work(&bus, "claude-code/held-running", generated()).await;
    sqlx::query("UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1")
        .bind(running)
        .execute(&pool)
        .await
        .expect("mark the thread running");

    let (_tmp_b, repo_b, archived) =
        thread_with_branch_work(&bus, "claude-code/held-archived", generated()).await;
    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(archived)
        .execute(&pool)
        .await
        .expect("archive the thread");

    let (_tmp_c, repo_c, failed) = thread_with_branch_work(
        &bus,
        "claude-code/held-failed",
        ThreadEvent::ResponseFailed {
            error: "stream interrupted".into(),
        },
    )
    .await;

    for (thread_id, repo, why) in [
        (running, &repo_a, "a running turn's own idle proposes"),
        (archived, &repo_b, "an archived thread's work is set aside"),
        (failed, &repo_c, "a failed turn proposes nothing"),
    ] {
        let scope = BranchWorkScope {
            pool: &pool,
            event_bus: &bus,
            lucidos_repo_root: repo,
            workspace_root: repo,
        };
        assert!(
            scope.work_a_canceled_wait_held(thread_id).await.is_none(),
            "{why}"
        );
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Only a cancel that leaves the thread open releases held work. A delivery or
/// an expiry re-opens the thread, whose next idle proposes, so the net would
/// propose twice. An archive or a discard ends the thread, and its own paths
/// settle the work.
#[test]
fn only_a_cancel_that_leaves_the_thread_open_releases_held_work() {
    let wait_id = Uuid::new_v4();
    let device = Some(MessageOrigin::Device {
        device_id: "test-device".into(),
    });
    let stopped_by = EventMeta::with_actor(device.clone());
    for cause in [
        EventWaitCancelCause::UserStop,
        EventWaitCancelCause::AgentStandDown,
    ] {
        assert_eq!(
            BranchWorkMoment::of(&wait_canceled(wait_id, cause), &stopped_by),
            Some(BranchWorkMoment::WaitCanceled(device.clone())),
            "{cause:?} releases the work, credited to whoever canceled"
        );
    }
    for cause in [
        EventWaitCancelCause::ThreadArchived,
        EventWaitCancelCause::ThreadDiscarded,
        EventWaitCancelCause::ThreadCanceled,
        EventWaitCancelCause::Unknown,
    ] {
        assert_eq!(
            BranchWorkMoment::of(&wait_canceled(wait_id, cause), &EventMeta::NONE),
            None,
            "{cause:?}"
        );
    }
    assert_eq!(
        BranchWorkMoment::of(&ThreadEvent::EventWaitExpired { wait_id }, &EventMeta::NONE),
        None
    );
    assert_eq!(
        BranchWorkMoment::of(&ThreadEvent::ThreadArchived, &EventMeta::NONE),
        Some(BranchWorkMoment::Archived)
    );
}

/// The net reads whether the parent's card waits on it BEFORE it proposes, and
/// sends the card AFTER. The proposal clears the deferral fact, so reading it
/// afterwards would strand the card. Source-text, like the run loop's ordering
/// pins: the property is the order of three calls.
#[test]
fn the_net_reads_the_card_deferral_before_proposing_and_sends_after() {
    const SRC: &str = include_str!("orphaned_branch_work.rs");
    let body = &SRC[SRC
        .find("pub(crate) async fn propose_work_a_canceled_wait_held(")
        .expect("the net's proposal entry point keeps its name")..];
    let at = |needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("the net must still call {needle}"))
    };
    let read = at("held_card_awaits_proposal(thread_id)");
    let propose = at("self.propose_thread_work(");
    let send = at("send_held_card_after_proposal(thread_id)");
    assert!(
        read < propose && propose < send,
        "read, then propose, then send"
    );
}

/// I5: withdrawing a change touches only its row, so the branch keeps every
/// commit. The archive net reads a withdrawn row as undecided: archiving the
/// thread afterwards sets the work aside, live and at boot.
#[tokio::test]
async fn the_archive_net_sets_aside_a_branch_whose_only_row_is_withdrawn() {
    use crate::engine::change_ops::emit_change_withdrawn;

    for at_boot in [false, true] {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let branch = "claude-code/withdrawn-then-archived";
        let (_tmp, repo, thread_id) = thread_with_branch_work(&bus, branch, generated()).await;
        let head = |repo: PathBuf| async move {
            String::from_utf8(
                git_cmd(&["rev-parse", branch], &repo)
                    .await
                    .expect("read the branch head")
                    .stdout,
            )
            .unwrap()
        };
        let before = head(repo.clone()).await;

        let withdrawn = Uuid::new_v4();
        let mut proposal = change_proposed(false);
        if let ThreadEvent::ChangeProposed {
            change_id,
            branch_name,
            ..
        } = &mut proposal
        {
            *change_id = withdrawn.to_string();
            *branch_name = branch.to_string();
        }
        bus.emit(BusEvent::Thread {
            thread_id,
            event: proposal,
            meta: cc_meta(),
        })
        .await
        .expect("propose");
        emit_change_withdrawn(&bus, thread_id, withdrawn, None)
            .await
            .expect("withdraw");
        assert_eq!(
            head(repo.clone()).await,
            before,
            "a withdrawal keeps the commits"
        );

        sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
            .bind(thread_id)
            .execute(&pool)
            .await
            .expect("archive the thread");
        let scope = BranchWorkScope {
            pool: &pool,
            event_bus: &bus,
            lucidos_repo_root: &repo,
            workspace_root: &repo,
        };
        if at_boot {
            assert_eq!(scope.set_aside_archived_branch_work_on_startup().await, 1);
        } else {
            assert!(scope
                .set_aside_archived_branch_work(thread_id)
                .await
                .expect("archive half runs"));
        }

        let projection = bus.changes_projection();
        let kept = projection
            .get_open_by_branch(branch)
            .await
            .unwrap()
            .expect("the work is set aside");
        assert_eq!(kept.status(), ChangeStatus::SetAside, "at_boot={at_boot}");
        assert_ne!(kept.id, withdrawn, "a withdrawn row is never resurrected");
        assert_eq!(
            projection
                .get_by_id(withdrawn)
                .await
                .unwrap()
                .expect("row")
                .status(),
            ChangeStatus::Withdrawn
        );

        pool.close().await;
        teardown_test_db(&db).await;
    }
}
