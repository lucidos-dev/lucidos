use super::*;
use crate::core::changes::ChangeStatus;
use crate::engine::event_bus::EmittedEvent;
use crate::engine::thread_events::EventWaitCancelCause;
use crate::engine::thread_lifecycle::CodingAgentChangeState;
use crate::test_support::{
    make_repo_and_worktree, read_change_state, setup_test_db, start_cc_session, teardown_test_db,
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

/// An integrated sub-thread proposes nothing, so its handed-up work has no
/// change row. Archiving its parent before the merge archives the child too.
/// The archive net then sets the child's work aside, as for any archived
/// thread (ADR 0465).
#[tokio::test]
async fn an_archived_integrated_childs_unmerged_work_is_set_aside() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/integrated-child";
    let (_tmp, repo, wt) = make_repo_and_worktree(branch).await;
    std::fs::write(wt.join("child.txt"), "follow-up").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "follow-up"], &wt).await.unwrap();

    let child = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: String::new(),
            branch: branch.into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: true,
            auto_compact_window: None,
            auto_compact_arm: None,
        },
        meta: cc_meta(),
    })
    .await
    .expect("start the integrated child");
    crate::engine::change_ops::emit_proposal_withheld(
        &bus,
        child,
        branch,
        &["child.txt".to_string()],
        UnproposedReason::IntegratedIntoParent,
    )
    .await;
    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(child)
        .execute(&pool)
        .await
        .expect("archive the child with its parent");

    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };
    assert!(scope
        .set_aside_archived_branch_work(child)
        .await
        .expect("archive half runs"));
    let kept = bus
        .changes_projection()
        .get_open_by_branch(branch)
        .await
        .unwrap()
        .expect("the child's work is set aside");
    assert_eq!(kept.status(), ChangeStatus::SetAside);
    assert_eq!(kept.files, vec!["child.txt".to_string()]);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A child of `parent` resting idle with one committed file on `branch`, in a
/// new worktree of `repo`. An integrated child holds that work for its parent,
/// as its hardened turn end does. Returns the child and its file.
async fn child_with_work(
    bus: &EventBus,
    pool: &PgPool,
    repo: &Path,
    parent: Uuid,
    branch: &str,
    integrated: bool,
) -> (Uuid, String) {
    let name = branch.replace('/', "-");
    let wt = repo.with_file_name(&name);
    git_cmd(
        &["worktree", "add", wt.to_str().unwrap(), "-b", branch],
        repo,
    )
    .await
    .unwrap();
    let file = format!("{name}.txt");
    std::fs::write(wt.join(&file), "child work").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "child work"], &wt).await.unwrap();

    let child = Uuid::new_v4();
    for event in [
        ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: String::new(),
            branch: branch.into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated,
            auto_compact_window: None,
            auto_compact_arm: None,
        },
        generated(),
    ] {
        bus.emit(BusEvent::Thread {
            thread_id: child,
            event,
            meta: cc_meta(),
        })
        .await
        .expect("run the child's turn");
    }
    if integrated {
        crate::engine::change_ops::emit_proposal_withheld(
            bus,
            child,
            branch,
            std::slice::from_ref(&file),
            UnproposedReason::IntegratedIntoParent,
        )
        .await;
    }
    sqlx::query("UPDATE thread_summaries SET parent_thread_id = $2 WHERE thread_id = $1")
        .bind(child)
        .bind(parent)
        .execute(pool)
        .await
        .expect("link the child");
    (child, file)
}

fn change_applied() -> ThreadEvent {
    ThreadEvent::ChangeApplied {
        change_id: Uuid::new_v4().to_string(),
        requires_restart: false,
        client_update: false,
        commits: Vec::new(),
        thread_title: None,
        actor: None,
        pre_merge_sha: None,
        post_merge_sha: None,
        landing_merge_sha: None,
        path: String::new(),
    }
}

async fn change_proposed_count(pool: &PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The parent's change landed without merging its integrated child's branch.
/// Nothing would ever propose that work, so the landing sets it aside. The
/// child's strip then stops saying its parent merges it (ADR 0465).
#[tokio::test]
async fn a_parents_landing_sets_aside_an_integrated_childs_unmerged_work() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, _parent_wt) = make_repo_and_worktree("claude-code/integrating-parent").await;
    let parent = Uuid::new_v4();
    let branch = "claude-code/unmerged-child";
    let (child, file) = child_with_work(&bus, &pool, &repo, parent, branch, true).await;
    assert_eq!(
        read_change_state(&pool, child).await,
        CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::IntegratedIntoParent)
        }
    );
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    assert_eq!(scope.set_aside_unmerged_integrated_work(parent).await, 1);
    let kept = bus
        .changes_projection()
        .get_open_by_branch(branch)
        .await
        .unwrap()
        .expect("the child's work is set aside");
    assert_eq!(kept.status(), ChangeStatus::SetAside);
    assert_eq!(kept.files, vec![file]);
    assert!(
        !kept.incomplete,
        "the child's turn finished, so Bring back makes the change pending"
    );
    let origin: serde_json::Value = sqlx::query_scalar(
        "SELECT payload->'origin' FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
    )
    .bind(child)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_value::<MessageOrigin>(origin).unwrap(),
        MessageOrigin::engine(EngineReason::UnmergedIntegratedWork)
    );
    assert_eq!(
        read_change_state(&pool, child).await,
        CodingAgentChangeState::Unproposed { reason: None },
        "the strip no longer says the parent merges the branch"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A second landing of the parent finds the set-aside row and adds nothing,
/// so a trigger on `ChangeProposed` hears the child's branch once.
#[tokio::test]
async fn a_second_parent_landing_adds_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, _parent_wt) = make_repo_and_worktree("claude-code/integrating-parent").await;
    let parent = Uuid::new_v4();
    let (child, _) =
        child_with_work(&bus, &pool, &repo, parent, "claude-code/landed-twice", true).await;
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    assert_eq!(scope.set_aside_unmerged_integrated_work(parent).await, 1);
    assert_eq!(scope.set_aside_unmerged_integrated_work(parent).await, 0);
    assert_eq!(change_proposed_count(&pool, child).await, 1);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A landing leaves four children alone:
/// - an integrated child whose branch the parent merged, which is on `main`;
/// - a running integrated child, whose report still wakes the parent;
/// - an idle one whose report the parent has not had yet;
/// - an independent child, which proposes its own work.
#[tokio::test]
async fn a_parents_landing_leaves_merged_live_and_independent_children_alone() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, _parent_wt) = make_repo_and_worktree("claude-code/integrating-parent").await;
    let parent = Uuid::new_v4();

    let merged_branch = "claude-code/merged-child";
    let (merged, _) = child_with_work(&bus, &pool, &repo, parent, merged_branch, true).await;
    git_cmd(&["merge", "--no-ff", merged_branch, "-m", "land"], &repo)
        .await
        .expect("land the child's commits on main");
    let (running, _) = child_with_work(
        &bus,
        &pool,
        &repo,
        parent,
        "claude-code/running-child",
        true,
    )
    .await;
    sqlx::query("UPDATE thread_summaries SET status = 'running' WHERE thread_id = $1")
        .bind(running)
        .execute(&pool)
        .await
        .expect("mark the child running");
    let (unreported, _) = child_with_work(
        &bus,
        &pool,
        &repo,
        parent,
        "claude-code/unreported-child",
        true,
    )
    .await;
    sqlx::query("UPDATE thread_summaries SET parent_callback_pending = TRUE WHERE thread_id = $1")
        .bind(unreported)
        .execute(&pool)
        .await
        .expect("owe the parent the child's report");
    let (independent, _) = child_with_work(
        &bus,
        &pool,
        &repo,
        parent,
        "claude-code/independent-child",
        false,
    )
    .await;
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    assert!(
        scope.thread_branch_work(merged).await.is_none(),
        "a branch on main has no net work"
    );
    assert_eq!(scope.set_aside_unmerged_integrated_work(parent).await, 0);
    for (child, why) in [
        (merged, "its commits landed"),
        (running, "its report still wakes the parent"),
        (unreported, "the parent still gets its report"),
        (independent, "it proposes its own work"),
    ] {
        assert_eq!(change_proposed_count(&pool, child).await, 0, "{why}");
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A lane thread archived mid-batch leaves work on its integration branch. The
/// landing lane lands or drops that branch, so the net reads it as no branch
/// work at all (ADR 0463). Live and at boot, it records nothing.
#[tokio::test]
async fn the_archive_net_skips_a_lane_threads_integration_branch() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = crate::engine::git_ops::lane_branch(Uuid::new_v4());
    let (_tmp, repo, thread_id) = thread_with_branch_work(&bus, &branch, generated()).await;
    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .expect("archive the lane thread");
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    assert!(
        scope.thread_branch_work(thread_id).await.is_none(),
        "a lane branch is no thread's branch work"
    );
    assert!(!scope
        .set_aside_archived_branch_work(thread_id)
        .await
        .expect("the archive half skips, it does not fail"));
    assert_eq!(scope.set_aside_archived_branch_work_on_startup().await, 0);
    let persisted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, 0, "no set-aside change names the lane branch");

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
            agent_exit: None,
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

/// The archived removal may commit a tree's dirt on its branch before the tree
/// goes. The net then sets that work aside, so it is listed, not just kept.
/// Stripping build artifacts moves no branch, so it wakes nothing.
#[test]
fn a_full_worktree_removal_sets_aside_what_it_left_on_the_branch() {
    let cleaned = |tier| ThreadEvent::WorktreeCleaned {
        tier,
        freed_bytes: 1,
        branch_deleted: false,
    };
    assert_eq!(
        BranchWorkMoment::of(&cleaned(2), &EventMeta::NONE),
        Some(BranchWorkMoment::Archived)
    );
    assert_eq!(BranchWorkMoment::of(&cleaned(1), &EventMeta::NONE), None);
}

/// Any landed change may leave an integrated sub-thread's work stranded, so
/// every `ChangeApplied` wakes the net for its thread.
#[test]
fn a_landed_change_wakes_the_net_for_its_integrated_children() {
    assert_eq!(
        BranchWorkMoment::of(&change_applied(), &EventMeta::NONE),
        Some(BranchWorkMoment::ChangeApplied)
    );
}

/// The replay list gates live frames too, so every event a moment comes from
/// must be on it.
#[test]
fn the_net_replays_every_event_a_moment_comes_from() {
    let moments = [
        ThreadEvent::ThreadArchived,
        ThreadEvent::WorktreeCleaned {
            tier: 2,
            freed_bytes: 1,
            branch_deleted: false,
        },
        wait_canceled(Uuid::new_v4(), EventWaitCancelCause::UserStop),
        change_applied(),
        ThreadEvent::ChangeDiscarded {
            change_id: Uuid::new_v4().to_string(),
            actor: None,
            path: String::new(),
        },
    ];
    for event in &moments {
        assert!(BranchWorkMoment::of(event, &EventMeta::NONE).is_some());
        assert!(BranchWorkMoment::EVENT_TYPES.contains(&event.event_type()));
    }
    assert_eq!(BranchWorkMoment::EVENT_TYPES.len(), moments.len());
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

/// Both moments lost to a broadcast lag must still reach the net. An archive's
/// work is set aside now, not at the next boot. A canceled wait's held work is
/// proposed, credited to whoever canceled it.
#[tokio::test]
async fn moments_lost_to_a_lag_still_reach_the_net() {
    use crate::engine::thread_events::EventMeta;
    use crate::test_support::{flood_past_bus_capacity, seed_chat_thread, seed_thread_event};

    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let (mut rx, mut replay) = LagReplay::subscribe(
        &bus,
        ReplayScope::Workspace,
        Some(BranchWorkMoment::EVENT_TYPES.to_vec()),
    );
    let archived = seed_chat_thread(&bus, "archive me").await;
    seed_thread_event(
        &bus,
        archived,
        ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;
    seed_thread_event(&bus, archived, ThreadEvent::ThreadArchived).await;
    let waiting = Uuid::new_v4();
    let wait_id = Uuid::new_v4();
    let on = vec![crate::core::event_subscription::EventSubscription {
        event_type: "ChangeProposed".into(),
        condition: None,
    }];
    seed_thread_event(
        &bus,
        waiting,
        ThreadEvent::EventWaitStarted {
            wait_id,
            tool_use_id: format!("toolu_{wait_id}"),
            on: on.clone(),
            reason: "watching the build".into(),
            armed_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            watermark: 0,
        },
    )
    .await;
    let canceled_by = MessageOrigin::Device {
        device_id: "test-device".into(),
    };
    bus.emit(BusEvent::Thread {
        thread_id: waiting,
        event: ThreadEvent::EventWaitCanceled {
            wait_id,
            cause: EventWaitCancelCause::UserStop,
            on,
            reason: "watching the build".into(),
        },
        meta: EventMeta::with_actor(Some(canceled_by.clone())),
    })
    .await
    .unwrap();
    flood_past_bus_capacity(&bus);

    let moments = moments_after(rx.recv().await, &mut replay, &pool)
        .await
        .expect("a lag must not end the net");
    assert_eq!(
        moments,
        vec![
            (archived, BranchWorkMoment::Archived),
            (waiting, BranchWorkMoment::WaitCanceled(Some(canceled_by))),
        ]
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A thread with one commit on `branch`, which handed off to a successor at
/// that commit. Returns the old thread, the successor and the worktree.
async fn handed_off_thread(
    bus: &EventBus,
    branch: &str,
) -> (Uuid, Uuid, tempfile::TempDir, PathBuf, PathBuf) {
    let (tmp, repo, wt) = make_repo_and_worktree(branch).await;
    std::fs::write(wt.join("carried.txt"), "carried work").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "carried work"], &wt)
        .await
        .unwrap();
    let base = branch_head_sha(&repo, branch)
        .await
        .expect("the branch has a head");

    let old = Uuid::new_v4();
    let successor = Uuid::new_v4();
    start_cc_session(bus, old, branch, None).await;
    bus.emit(BusEvent::Thread {
        thread_id: old,
        event: ThreadEvent::SuccessorThreadSpawned {
            successor_thread_id: successor,
            base_sha: base,
            starts_build_chain: false,
            handoff_consent: None,
        },
        meta: cc_meta(),
    })
    .await
    .expect("record the handoff");
    bus.emit(BusEvent::Thread {
        thread_id: successor,
        event: crate::test_support::message_received("carry on"),
        meta: cc_meta(),
    })
    .await
    .expect("start the successor");
    (old, successor, tmp, repo, wt)
}

/// I15: archiving the old thread before its successor's change lands sets
/// nothing aside. The successor's branch carries the work, and its change
/// owns it.
#[tokio::test]
async fn work_a_successor_carries_is_never_set_aside() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/handed-off";
    let (old, successor, _tmp, repo, _wt) = handed_off_thread(&bus, branch).await;
    let successor_branch = "claude-code/successor";
    git_cmd(&["branch", successor_branch, branch], &repo)
        .await
        .unwrap();
    start_cc_session(&bus, successor, successor_branch, None).await;
    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    assert!(!scope.set_aside_archived_branch_work(old).await.unwrap());
    assert!(!scope.set_aside_successor_residue(successor).await);
    assert_eq!(change_proposed_count(&pool, old).await, 0);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// I15: a commit the old thread made after the handoff is residue. The
/// successor never saw it, so its landing sets it aside rather than lose it.
/// The successor has no branch yet, so ancestry is unknown, and unknown sets
/// aside.
#[tokio::test]
async fn work_after_the_handoff_is_set_aside_when_the_successor_lands() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/kept-working";
    let (old, successor, _tmp, repo, wt) = handed_off_thread(&bus, branch).await;
    std::fs::write(wt.join("residue.txt"), "after the handoff").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "residue"], &wt).await.unwrap();
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    assert!(scope.set_aside_successor_residue(successor).await);
    assert_eq!(change_proposed_count(&pool, old).await, 1);
    assert!(
        !scope.set_aside_successor_residue(successor).await,
        "a second landing adds nothing"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// I15: a successor whose spawn never made a branch carries nothing. So the
/// old thread's archive sets its work aside rather than strand it.
#[tokio::test]
async fn a_successor_with_no_branch_carries_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/stranded";
    let (old, _successor, _tmp, repo, _wt) = handed_off_thread(&bus, branch).await;
    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    assert!(scope.set_aside_archived_branch_work(old).await.unwrap());
    assert_eq!(change_proposed_count(&pool, old).await, 1);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A Discard resets the successor's branch to main. The old thread's work at
/// the handoff was the successor's to decide. So the Discard covers it, and
/// nothing comes back as a set-aside change.
#[tokio::test]
async fn a_discarded_successor_brings_nothing_back() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/discarded-successor";
    let (old, successor, _tmp, repo, _wt) = handed_off_thread(&bus, branch).await;
    let successor_branch = "claude-code/reset-to-main";
    git_cmd(&["branch", successor_branch, "main"], &repo)
        .await
        .unwrap();
    start_cc_session(&bus, successor, successor_branch, None).await;
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    assert!(!scope.set_aside_successor_residue(successor).await);
    assert_eq!(change_proposed_count(&pool, old).await, 0);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A set-aside change from before the handoff is still open on the branch,
/// and a branch holds one open change. So the residue pass adds none.
#[tokio::test]
async fn an_open_change_from_before_the_handoff_keeps_the_residue_pass_quiet() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/set-aside-then-handed-off";
    let (old, successor, _tmp, repo, wt) = handed_off_thread(&bus, branch).await;
    sqlx::query(
        "INSERT INTO changes (id, request_id, thread_id, branch_name, repo_root, status, created_at) \
         VALUES (gen_random_uuid(), gen_random_uuid(), $1, $2, $3, 'set_aside', now() - interval '1 hour')",
    )
    .bind(old)
    .bind(branch)
    .bind(repo.to_string_lossy().to_string())
    .execute(&pool)
    .await
    .expect("record a change set aside before the handoff");
    std::fs::write(wt.join("residue.txt"), "after the handoff").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "residue"], &wt).await.unwrap();
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    assert!(!scope.set_aside_successor_residue(successor).await);
    assert_eq!(change_proposed_count(&pool, old).await, 0);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// I15: a change applied on the branch before the handoff does not stop the
/// residue after it from being set aside. Only a decision since the handoff
/// covers the residue.
#[tokio::test]
async fn residue_is_set_aside_even_after_an_earlier_apply() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/applied-then-handed-off";
    let (old, successor, _tmp, repo, wt) = handed_off_thread(&bus, branch).await;
    let change_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO changes (id, request_id, thread_id, branch_name, repo_root, status, created_at) \
         VALUES ($1::uuid, $1::uuid, $2, $3, $4, 'applied', now() - interval '1 hour')",
    )
    .bind(&change_id)
    .bind(old)
    .bind(branch)
    .bind(repo.to_string_lossy().to_string())
    .execute(&pool)
    .await
    .expect("record the change applied before the handoff");
    std::fs::write(wt.join("residue.txt"), "after the handoff").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "residue"], &wt).await.unwrap();
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    assert!(scope.set_aside_successor_residue(successor).await);
    assert_eq!(change_proposed_count(&pool, old).await, 1);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The successor's resolved change wakes the residue pass, applied or
/// discarded.
#[test]
fn a_resolved_change_wakes_the_residue_pass() {
    assert_eq!(
        BranchWorkMoment::of(
            &ThreadEvent::ChangeDiscarded {
                change_id: Uuid::new_v4().to_string(),
                actor: None,
                path: String::new(),
            },
            &EventMeta::NONE
        ),
        Some(BranchWorkMoment::ChangeDiscarded)
    );
}

/// I15: commits after the handoff that the successor's branch already holds
/// are carried, not residue. Git confirms the old head is in that branch.
#[tokio::test]
async fn work_the_successors_branch_holds_is_never_residue() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/merged-forward";
    let (old, successor, _tmp, repo, wt) = handed_off_thread(&bus, branch).await;
    std::fs::write(wt.join("later.txt"), "after the handoff").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "later"], &wt).await.unwrap();
    let old_head = branch_head_sha(&repo, branch).await.expect("a head");
    git_cmd(&["branch", "claude-code/successor", &old_head], &repo)
        .await
        .unwrap();
    start_cc_session(&bus, successor, "claude-code/successor", None).await;
    let scope = BranchWorkScope {
        pool: &pool,
        event_bus: &bus,
        lucidos_repo_root: &repo,
        workspace_root: &repo,
    };

    assert!(!scope.set_aside_successor_residue(successor).await);
    assert_eq!(change_proposed_count(&pool, old).await, 0);

    pool.close().await;
    teardown_test_db(&db).await;
}
