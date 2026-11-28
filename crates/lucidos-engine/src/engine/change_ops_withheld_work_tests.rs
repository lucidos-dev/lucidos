use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::git_ops::git_cmd;
use crate::engine::thread_events::{CancelCause, EventChannel, EventMeta, ThreadEvent};
use crate::engine::thread_lifecycle::CodingAgentChangeState;
use crate::test_support::{
    make_repo_and_worktree, read_change_state, setup_test_db, start_cc_session, teardown_test_db,
};

const BRANCH: &str = "claude-code/withheld-work";

async fn thread_with_work(bus: &EventBus) -> (tempfile::TempDir, PathBuf, Uuid) {
    let (tmp, repo, wt) = make_repo_and_worktree(BRANCH).await;
    std::fs::write(wt.join("a.txt"), "work").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "work"], &wt).await.unwrap();
    let thread_id = Uuid::new_v4();
    start_cc_session(bus, thread_id, BRANCH, None).await;
    (tmp, repo, thread_id)
}

fn input<'a>(
    thread_id: Uuid,
    repo_root: &'a str,
    description: &'a str,
    files: &'a [String],
) -> ProposeChangeInput<'a> {
    ProposeChangeInput {
        thread_id,
        branch_name: BRANCH,
        repo_root,
        description,
        files,
        requires_restart: false,
        channel: EventChannel::ClaudeCode,
        hardened: false,
        origin: None,
    }
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("emit");
}

fn proposed(change_id: Uuid, description: &str, files: &[String], set_aside: bool) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        content: Default::default(),
        change_id: change_id.to_string(),
        description: Some(description.into()),
        files: files.to_vec(),
        requires_restart: false,
        origin: None,
        commit_sha: None,
        branch_name: BRANCH.into(),
        repo_root: "/repo".into(),
        hardened: false,
        incomplete: set_aside,
        set_aside,
        path: String::new(),
        diff: String::new(),
    }
}

async fn payloads(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    event_type: &str,
) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT payload FROM events WHERE thread_id = $1 AND event_type = $2 ORDER BY sequence",
    )
    .bind(thread_id)
    .bind(event_type)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn status_of(pool: &sqlx::PgPool, change_id: Uuid) -> ChangeStatus {
    sqlx::query_scalar("SELECT status FROM changes WHERE id = $1")
        .bind(change_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn files(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

async fn hold_of(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    input: &ProposeChangeInput<'_>,
) -> Option<ProposalHold> {
    match withhold_unready(
        pool,
        bus,
        input,
        crate::engine::change_ops::HoldReentry::default(),
    )
    .await
    .expect("withhold")
    {
        HoldOutcome::Clear => None,
        HoldOutcome::Held(hold, _) => Some(hold),
    }
}

#[tokio::test]
async fn a_plan_hold_announces_the_branch_files_and_reason_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, thread_id) = thread_with_work(&bus).await;
    let repo_str = repo.to_string_lossy().to_string();
    let work = files(&["a.txt"]);

    let hold = hold_of(&pool, &bus, &input(thread_id, &repo_str, "work", &work)).await;

    assert_eq!(hold, Some(ProposalHold::Plan(PlanHold::Missing)));
    let withheld = payloads(&pool, thread_id, "ProposalWithheld").await;
    assert_eq!(withheld.len(), 1, "{withheld:?}");
    assert_eq!(withheld[0]["branch_name"], BRANCH);
    assert_eq!(withheld[0]["files"], serde_json::json!(["a.txt"]));
    assert_eq!(withheld[0]["reason"], "plan_missing");
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::PlanMissing)
        }
    );

    crate::engine::git_ops::record_planned(
        &pool,
        &repo,
        BRANCH,
        crate::engine::git_ops::PlanMarkerKind::AcknowledgedSimple,
        None,
        Some("test fixture"),
        &[],
        "abc",
    )
    .await
    .expect("record a satisfying plan marker");
    crate::engine::git_ops::record_hardened(
        &pool,
        &repo,
        BRANCH,
        "abc",
        crate::engine::git_ops::HardenTier::Full,
    )
    .await
    .expect("record the harden marker");
    assert_eq!(
        hold_of(&pool, &bus, &input(thread_id, &repo_str, "work", &work)).await,
        None
    );
    assert_eq!(
        payloads(&pool, thread_id, "ProposalWithheld").await.len(),
        1
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn unhardened_work_is_held_but_a_plan_alone_is_not() {
    use crate::engine::git_ops::{record_hardened, record_planned, PlanMarkerKind};

    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, thread_id) = thread_with_work(&bus).await;
    let repo_str = repo.to_string_lossy().to_string();
    let mark = |kind| record_planned(&pool, &repo, BRANCH, kind, None, None, &[], "abc");
    mark(PlanMarkerKind::Planned).await.expect("mark planned");

    let code = files(&["a.txt"]);
    let hold = hold_of(&pool, &bus, &input(thread_id, &repo_str, "work", &code)).await;
    assert_eq!(hold, Some(ProposalHold::HardeningMissing));
    let withheld = payloads(&pool, thread_id, "ProposalWithheld").await;
    assert_eq!(withheld.last().unwrap()["reason"], "hardening_missing");
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::HardeningMissing)
        }
    );

    let plan = files(&["docs/plans/2026-10-10-x.md"]);
    mark(PlanMarkerKind::Planned).await.expect("mark the plan");
    assert_eq!(
        hold_of(&pool, &bus, &input(thread_id, &repo_str, "plan", &plan)).await,
        None,
        "an approved plan alone is never held for hardening"
    );
    mark(PlanMarkerKind::Proposed).await.expect("mark the plan");
    assert_eq!(
        hold_of(&pool, &bus, &input(thread_id, &repo_str, "plan", &plan)).await,
        Some(ProposalHold::Plan(PlanHold::PlanOnlyAwaitingApproval)),
        "a plan awaiting approval is held, not proposed"
    );
    let withheld = payloads(&pool, thread_id, "ProposalWithheld").await;
    assert_eq!(
        withheld.last().expect("a plan-only hold announces")["reason"],
        "plan_awaiting_approval"
    );

    record_hardened(
        &pool,
        &repo,
        BRANCH,
        "abc",
        crate::engine::git_ops::HardenTier::Full,
    )
    .await
    .expect("record the harden marker");
    mark(PlanMarkerKind::Planned).await.expect("mark planned");
    assert_eq!(
        hold_of(&pool, &bus, &input(thread_id, &repo_str, "work", &code)).await,
        None
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn handed_off_work_withdraws_the_pending_change_unannounced() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, thread_id) = thread_with_work(&bus).await;
    let plan = files(&["docs/plans/x.md"]);
    let change_id = Uuid::new_v4();
    emit(&bus, thread_id, proposed(change_id, "plan", &plan, false)).await;

    let withdrew = withhold_handed_off_work(&pool, &bus, thread_id, &repo, BRANCH)
        .await
        .expect("withhold");

    assert!(withdrew);
    assert_eq!(status_of(&pool, change_id).await, ChangeStatus::Withdrawn);
    assert!(payloads(&pool, thread_id, "ProposalWithheld")
        .await
        .is_empty());
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Unproposed { reason: None },
        "no Apply, Review or Not ready strip before the successor session starts"
    );
    assert!(
        !withhold_handed_off_work(&pool, &bus, thread_id, &repo, BRANCH)
            .await
            .expect("withhold")
    );
    assert_eq!(payloads(&pool, thread_id, "ChangeWithdrawn").await.len(), 1);

    let parked = Uuid::new_v4();
    emit(&bus, thread_id, proposed(parked, "plan", &plan, true)).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: Some(branch_head(&repo).await),
            bg_bash_pending: false,
            turn_end: None,
        },
    )
    .await;
    assert!(
        !withhold_handed_off_work(&pool, &bus, thread_id, &repo, BRANCH)
            .await
            .expect("withhold")
    );
    assert_eq!(
        status_of(&pool, parked).await,
        ChangeStatus::SetAside,
        "a set-aside change stays where the user put it"
    );

    let wt = repo.parent().unwrap().join("wt");
    std::fs::write(wt.join("b.txt"), "more").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "more"], &wt).await.unwrap();
    assert!(
        withhold_handed_off_work(&pool, &bus, thread_id, &repo, BRANCH)
            .await
            .expect("withhold")
    );
    assert_eq!(status_of(&pool, parked).await, ChangeStatus::Withdrawn);

    pool.close().await;
    teardown_test_db(&db).await;
}

async fn branch_head(repo: &std::path::Path) -> String {
    String::from_utf8(git_cmd(&["rev-parse", BRANCH], repo).await.unwrap().stdout)
        .unwrap()
        .trim()
        .to_string()
}

#[tokio::test]
async fn an_integrated_child_hands_up_hardened_work_and_never_proposes() {
    use crate::engine::git_ops::{record_hardened, record_planned, HardenTier, PlanMarkerKind};

    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, wt) = make_repo_and_worktree(BRANCH).await;
    std::fs::write(wt.join("a.txt"), "work").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "work"], &wt).await.unwrap();
    let thread_id = Uuid::new_v4();
    emit(
        &bus,
        thread_id,
        ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: String::new(),
            branch: BRANCH.into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: true,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
    )
    .await;
    record_planned(
        &pool,
        &repo,
        BRANCH,
        PlanMarkerKind::AcknowledgedSimple,
        None,
        Some("test fixture"),
        &[],
        "abc",
    )
    .await
    .expect("record a satisfying plan marker");
    let repo_str = repo.to_string_lossy().to_string();
    let code = files(&["a.txt"]);
    let work = input(thread_id, &repo_str, "work", &code);
    let hold = || hold_of(&pool, &bus, &work);

    assert_eq!(hold().await, Some(ProposalHold::HardeningMissing));

    let head = branch_head(&repo).await;
    record_hardened(&pool, &repo, BRANCH, "stale", HardenTier::Fast)
        .await
        .expect("record a stale fast marker");
    assert_eq!(
        hold().await,
        Some(ProposalHold::HardeningMissing),
        "a stale marker hands nothing up"
    );

    for tier in [HardenTier::Fast, HardenTier::Full] {
        record_hardened(&pool, &repo, BRANCH, &head, tier)
            .await
            .expect("record a fresh marker");
        assert_eq!(
            hold().await,
            Some(ProposalHold::IntegratedIntoParent),
            "{tier:?}"
        );
        assert_eq!(
            read_change_state(&pool, thread_id).await,
            CodingAgentChangeState::Unproposed {
                reason: Some(UnproposedReason::IntegratedIntoParent)
            }
        );
    }
    let withheld = payloads(&pool, thread_id, "ProposalWithheld").await;
    assert_eq!(withheld.last().unwrap()["reason"], "integrated_into_parent");
    assert!(payloads(&pool, thread_id, "ChangeProposed")
        .await
        .is_empty());

    let plan = files(&["docs/plans/2026-10-13-x.md"]);
    assert_eq!(
        hold_of(&pool, &bus, &input(thread_id, &repo_str, "plan", &plan)).await,
        None
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_independent_branch_with_only_a_fast_marker_is_not_hardened() {
    use crate::engine::git_ops::{
        has_full_harden_marker, record_hardened, record_planned, HardenTier, PlanMarkerKind,
    };

    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, thread_id) = thread_with_work(&bus).await;
    record_planned(
        &pool,
        &repo,
        BRANCH,
        PlanMarkerKind::AcknowledgedSimple,
        None,
        Some("test fixture"),
        &[],
        "abc",
    )
    .await
    .expect("record a satisfying plan marker");
    let head = branch_head(&repo).await;
    record_hardened(&pool, &repo, BRANCH, &head, HardenTier::Fast)
        .await
        .expect("record the fast marker");
    let repo_str = repo.to_string_lossy().to_string();
    let code = files(&["a.txt"]);

    assert_eq!(
        hold_of(&pool, &bus, &input(thread_id, &repo_str, "work", &code)).await,
        Some(ProposalHold::HardeningMissing)
    );
    let kind = load_apply_kind_context(&pool, Some(thread_id)).await;
    let hardened = has_full_harden_marker(&pool, &repo, BRANCH).await;
    assert!(!hardened, "a fast marker is no full marker");
    assert!(
        apply_must_harden(&kind, hardened, Some(&code)),
        "Apply hardens a branch carrying only a fast marker"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_unfinished_turn_past_a_pending_change_withdraws_it() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, thread_id) = thread_with_work(&bus).await;
    let change_id = Uuid::new_v4();
    emit(
        &bus,
        thread_id,
        proposed(change_id, "first", &files(&["a.txt"]), false),
    )
    .await;
    let head =
        String::from_utf8(git_cmd(&["rev-parse", BRANCH], &repo).await.unwrap().stdout).unwrap();

    let further = files(&["a.txt", "b.txt"]);
    let withdrew = withhold_unfinished_work(
        &bus,
        &input(thread_id, "/repo", "first\nsecond", &further),
        false,
    )
    .await
    .expect("withhold");

    assert!(withdrew);
    assert_eq!(status_of(&pool, change_id).await, ChangeStatus::Withdrawn);
    assert_eq!(payloads(&pool, thread_id, "ChangeWithdrawn").await.len(), 1);
    let withheld = payloads(&pool, thread_id, "ProposalWithheld").await;
    assert_eq!(withheld.len(), 1);
    assert_eq!(withheld[0]["reason"], "turn_incomplete");
    assert_eq!(withheld[0]["files"], serde_json::json!(["a.txt", "b.txt"]));
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::TurnIncomplete)
        }
    );
    assert!(bus
        .changes_projection()
        .pending_for_thread(thread_id)
        .await
        .unwrap()
        .is_empty());
    let after =
        String::from_utf8(git_cmd(&["rev-parse", BRANCH], &repo).await.unwrap().stdout).unwrap();
    assert_eq!(after, head, "withdrawing touches only the changes row");

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_unfinished_turn_with_no_new_commits_keeps_the_pending_change() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, _repo, thread_id) = thread_with_work(&bus).await;
    let change_id = Uuid::new_v4();
    let work = files(&["a.txt"]);
    emit(&bus, thread_id, proposed(change_id, "first", &work, false)).await;

    let withdrew =
        withhold_unfinished_work(&bus, &input(thread_id, "/repo", "first", &work), false)
            .await
            .expect("withhold");

    assert!(!withdrew);
    assert_eq!(status_of(&pool, change_id).await, ChangeStatus::Pending);
    assert!(payloads(&pool, thread_id, "ChangeWithdrawn")
        .await
        .is_empty());
    assert!(payloads(&pool, thread_id, "ProposalWithheld")
        .await
        .is_empty());
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Proposed {
            requires_restart: false
        }
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_unfinished_turn_that_moved_the_branch_withdraws_even_with_the_same_files() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, _repo, thread_id) = thread_with_work(&bus).await;
    let change_id = Uuid::new_v4();
    let work = files(&["a.txt"]);
    emit(&bus, thread_id, proposed(change_id, "first", &work, false)).await;

    assert!(
        withhold_unfinished_work(&bus, &input(thread_id, "/repo", "first", &work), true)
            .await
            .expect("withhold")
    );

    assert_eq!(status_of(&pool, change_id).await, ChangeStatus::Withdrawn);
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::TurnIncomplete)
        }
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_unfinished_turn_withdraws_a_set_aside_change_it_moved_past() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, _repo, thread_id) = thread_with_work(&bus).await;
    let change_id = Uuid::new_v4();
    let work = files(&["a.txt"]);
    emit(&bus, thread_id, proposed(change_id, "first", &work, false)).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::ChangeSetAside {
            change_id: change_id.to_string(),
            hold_id: None,
        },
    )
    .await;

    assert!(
        withhold_unfinished_work(&bus, &input(thread_id, "/repo", "first", &work), true)
            .await
            .expect("withhold")
    );

    assert_eq!(status_of(&pool, change_id).await, ChangeStatus::Withdrawn);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_branch_moved_unless_the_last_idle_recorded_its_head() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, thread_id) = thread_with_work(&bus).await;
    let head = String::from_utf8(git_cmd(&["rev-parse", BRANCH], &repo).await.unwrap().stdout)
        .unwrap()
        .trim()
        .to_string();
    let idle = |sha: Option<&str>| ThreadEvent::CodingAgentIdled {
        has_changes: true,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: sha.map(str::to_string),
        bg_bash_pending: false,
        turn_end: None,
    };

    assert!(branch_moved_since_last_idle(&pool, thread_id, &repo, BRANCH).await);
    emit(&bus, thread_id, idle(Some(&head))).await;
    assert!(!branch_moved_since_last_idle(&pool, thread_id, &repo, BRANCH).await);
    emit(&bus, thread_id, idle(None)).await;
    assert!(
        !branch_moved_since_last_idle(&pool, thread_id, &repo, BRANCH).await,
        "an engine idle with no head is skipped"
    );
    emit(
        &bus,
        thread_id,
        idle(Some("0000000000000000000000000000000000000000")),
    )
    .await;
    assert!(branch_moved_since_last_idle(&pool, thread_id, &repo, BRANCH).await);
    assert!(branch_moved_since_last_idle(&pool, thread_id, Path::new("/repo"), BRANCH).await);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_unfinished_turn_never_creates_a_pending_change() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, _repo, thread_id) = thread_with_work(&bus).await;
    let work = files(&["a.txt"]);

    assert!(
        !withhold_unfinished_work(&bus, &input(thread_id, "/repo", "work", &work), false)
            .await
            .expect("withhold")
    );

    assert!(payloads(&pool, thread_id, "ChangeProposed")
        .await
        .is_empty());
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM changes WHERE branch_name = $1")
        .bind(BRANCH)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::TurnIncomplete)
        }
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn bring_back_withdraws_an_incomplete_set_aside_row_and_redecides_it() {
    let stopped = ThreadEvent::ResponseCanceled {
        text: String::new(),
        images: Vec::new(),
        model: None,
        reasoning_effort: None,
        cause: CancelCause::UserStop,
    };
    let finished = ThreadEvent::ResponseGenerated {
        text: "Done.".into(),
        images: Vec::new(),
        model: None,
        reasoning_effort: None,
    };
    for (terminal, expected_finished) in [(stopped, false), (finished, true)] {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let (_tmp, _repo, thread_id) = thread_with_work(&bus).await;
        let work = files(&["a.txt"]);
        emit(&bus, thread_id, terminal).await;
        emit(&bus, thread_id, ThreadEvent::ThreadArchived).await;
        let change_id = Uuid::new_v4();
        emit(&bus, thread_id, proposed(change_id, "work", &work, true)).await;
        assert_eq!(status_of(&pool, change_id).await, ChangeStatus::SetAside);

        let turn_finished = withdraw_for_redecision(&pool, &bus, thread_id, change_id, None)
            .await
            .expect("withdraw");

        assert_eq!(turn_finished, expected_finished);
        assert_eq!(status_of(&pool, change_id).await, ChangeStatus::Withdrawn);
        let archive_state: String =
            sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
                .bind(thread_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(archive_state, "inbox", "the work needs a decision again");

        if !turn_finished {
            withhold_unfinished_work(&bus, &input(thread_id, "/repo", "work", &work), false)
                .await
                .expect("withhold");
            assert!(bus
                .changes_projection()
                .pending_for_thread(thread_id)
                .await
                .unwrap()
                .is_empty());
            assert_eq!(
                read_change_state(&pool, thread_id).await,
                CodingAgentChangeState::Unproposed {
                    reason: Some(UnproposedReason::TurnIncomplete)
                }
            );
        }

        pool.close().await;
        teardown_test_db(&db).await;
    }
}

#[test]
fn bring_back_redecides_an_incomplete_row_instead_of_restoring_it() {
    let src = include_str!("change_ops/set_aside.rs");
    let body = &src[src.find("async fn bring_back_change(").unwrap()..];
    let incomplete = &body[body.find("if change.incomplete {").unwrap()..];
    let arm = &incomplete[..incomplete.find("return Ok(());").unwrap()];
    let withdraw = arm
        .find("withdraw_for_redecision(")
        .expect("withdraws first");
    let host = arm.find("self.agent_host()").expect("calls the agent host");
    let redecide = arm.find(".redecide_branch_work(").expect("then re-decides");
    assert!(withdraw < host && host < redecide);
    let read = arm
        .find("branch_changed_files_checked(")
        .expect("reads the branch first");
    assert!(
        read < withdraw,
        "a failed read must leave the row set aside"
    );
    assert!(
        arm[redecide..].contains("finished"),
        "the re-decision takes the turn's answer"
    );
    assert!(!arm.contains("ChangeBroughtBack"));
}

#[test]
fn propose_turn_work_withholds_every_unfinished_turn() {
    let src = include_str!("change_ops/propose.rs");
    let body = &src[src.find("async fn propose_turn_work(").unwrap()..];
    let body = &body[..body.find("\n    }\n").unwrap()];
    let finished_arm = body
        .find("if finished {")
        .expect("a finished turn proposes");
    let withhold = body
        .find("withhold_unfinished_work(&self.event_bus, &input, moved)")
        .expect("an unfinished one withholds");
    assert!(finished_arm < withhold);
    assert_eq!(body.matches("propose_change(").count(), 1);
}

#[tokio::test]
async fn a_hold_withdraws_an_open_change_the_turn_committed_past() {
    use crate::engine::git_ops::{record_planned, PlanMarkerKind};

    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, thread_id) = thread_with_work(&bus).await;
    let repo_str = repo.to_string_lossy().to_string();
    record_planned(
        &pool,
        &repo,
        BRANCH,
        PlanMarkerKind::Planned,
        None,
        None,
        &[],
        "abc",
    )
    .await
    .expect("mark planned");
    let change_id = Uuid::new_v4();
    let work = files(&["a.txt"]);
    let mut hardened = proposed(change_id, "work", &work, false);
    if let ThreadEvent::ChangeProposed { hardened: h, .. } = &mut hardened {
        *h = true;
    }
    emit(&bus, thread_id, hardened).await;
    let head = String::from_utf8(git_cmd(&["rev-parse", BRANCH], &repo).await.unwrap().stdout)
        .unwrap()
        .trim()
        .to_string();
    emit(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: Some(head),
            bg_bash_pending: false,
            turn_end: None,
        },
    )
    .await;

    let same = withhold_unready(
        &pool,
        &bus,
        &input(thread_id, &repo_str, "work", &work),
        crate::engine::change_ops::HoldReentry::default(),
    )
    .await
    .expect("withhold");
    assert_eq!(
        same,
        HoldOutcome::Held(
            ProposalHold::HardeningMissing,
            OpenChangeAfterHold::StillCarries(change_id)
        )
    );
    assert_eq!(status_of(&pool, change_id).await, ChangeStatus::Pending);

    let more = files(&["a.txt", "b.txt"]);
    let moved = withhold_unready(
        &pool,
        &bus,
        &input(thread_id, &repo_str, "more", &more),
        crate::engine::change_ops::HoldReentry::default(),
    )
    .await
    .expect("withhold");
    assert_eq!(
        moved,
        HoldOutcome::Held(
            ProposalHold::HardeningMissing,
            OpenChangeAfterHold::Withdrawn
        )
    );
    assert_eq!(status_of(&pool, change_id).await, ChangeStatus::Withdrawn);
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::HardeningMissing)
        }
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn work_after_a_queued_change_waits_for_its_landing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (tmp, repo, thread_id) = thread_with_work(&bus).await;
    let repo_str = repo.to_string_lossy().to_string();
    let work = files(&["a.txt"]);
    let change_id = Uuid::new_v4();
    emit(&bus, thread_id, proposed(change_id, "work", &work, false)).await;
    let snapshot = String::from_utf8(git_cmd(&["rev-parse", BRANCH], &repo).await.unwrap().stdout)
        .unwrap()
        .trim()
        .to_string();
    emit(
        &bus,
        thread_id,
        ThreadEvent::ChangeQueued {
            change_id: change_id.to_string(),
            snapshot_sha: snapshot,
        },
    )
    .await;

    let at_snapshot =
        withhold_for_landing(&pool, &bus, &input(thread_id, &repo_str, "work", &work))
            .await
            .expect("read the lane");
    assert_eq!(at_snapshot, Some(change_id));
    assert!(
        payloads(&pool, thread_id, "ProposalWithheld")
            .await
            .is_empty(),
        "nothing new to withhold"
    );

    let wt = tmp.path().join("wt");
    std::fs::write(wt.join("b.txt"), "more").unwrap();
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "more"], &wt).await.unwrap();
    let more = files(&["a.txt", "b.txt"]);
    let moved = withhold_for_landing(&pool, &bus, &input(thread_id, &repo_str, "more", &more))
        .await
        .expect("read the lane");
    assert_eq!(moved, Some(change_id));
    let withheld = payloads(&pool, thread_id, "ProposalWithheld").await;
    assert_eq!(withheld.len(), 1);
    assert_eq!(withheld[0]["reason"], "awaiting_landing");
    assert!(payloads(&pool, thread_id, "ChangeWithdrawn")
        .await
        .is_empty());
    assert_eq!(status_of(&pool, change_id).await, ChangeStatus::Pending);

    emit(
        &bus,
        thread_id,
        ThreadEvent::ChangeApplied {
            content: Default::default(),
            change_id: change_id.to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
            path: String::new(),
        },
    )
    .await;
    let landed = withhold_for_landing(&pool, &bus, &input(thread_id, &repo_str, "more", &more))
        .await
        .expect("read the lane");
    assert_eq!(landed, None, "the landed change frees the next proposal");

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_hold_the_agent_will_reenter_announces_no_owner_action() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, thread_id) = thread_with_work(&bus).await;
    let repo_str = repo.to_string_lossy().to_string();
    let work = files(&["a.txt"]);
    let outcome = withhold_unready(
        &pool,
        &bus,
        &input(thread_id, &repo_str, "work", &work),
        HoldReentry {
            plan: true,
            hardening: false,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        outcome,
        HoldOutcome::Held(ProposalHold::Plan(PlanHold::Missing), _)
    ));
    let withheld = payloads(&pool, thread_id, "ProposalWithheld").await;
    assert_eq!(withheld[0]["reason"], "plan_missing");
    assert_eq!(withheld[0]["agent_returns"], true);
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Unproposed { reason: None }
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_refusing_hook_withholds_a_proposal_and_keeps_its_sibling() {
    use crate::engine::git_ops::{record_hardened, record_planned, HardenTier, PlanMarkerKind};
    use crate::test_support::{hook_refusals, install_refusing_hook};

    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = crate::engine_test_support::boot_mock_engine(&db).await;
    let bus = &engine.event_bus;
    let (_tmp, repo, thread_id) = thread_with_work(bus).await;
    let repo_str = repo.to_string_lossy().to_string();
    record_planned(
        &pool,
        &repo,
        BRANCH,
        PlanMarkerKind::Planned,
        None,
        None,
        &[],
        "abc",
    )
    .await
    .expect("mark planned");
    record_hardened(&pool, &repo, BRANCH, "abc", HardenTier::Full)
        .await
        .expect("record the harden marker");
    let sibling = Uuid::new_v4();
    let mut on_sibling = proposed(sibling, "older", &files(&["old.txt"]), false);
    if let ThreadEvent::ChangeProposed { branch_name, .. } = &mut on_sibling {
        *branch_name = "claude-code/sibling".into();
    }
    emit(bus, thread_id, on_sibling).await;
    let open = Uuid::new_v4();
    emit(
        bus,
        thread_id,
        proposed(open, "work", &files(&["a.txt"]), false),
    )
    .await;
    let asker = install_refusing_hook(bus, "require-a-plan", "ChangeProposed", "No plan.").await;

    let more = files(&["a.txt", "b.txt"]);
    let mut asked_input = input(thread_id, &repo_str, "more", &more);
    asked_input.hardened = true;
    let outcome = engine
        .propose_change(asked_input, HoldReentry::default())
        .await
        .expect("propose");

    let ProposeOutcome::Refused(refusal) = outcome else {
        panic!("a refusing hook stops the proposal: {outcome:?}");
    };
    assert_eq!(refusal.reason, "No plan.");
    assert_eq!(asker.asked().len(), 1);
    assert_eq!(payloads(&pool, thread_id, "ChangeProposed").await.len(), 2);
    assert_eq!(status_of(&pool, sibling).await, ChangeStatus::Pending);
    assert_eq!(status_of(&pool, open).await, ChangeStatus::Withdrawn);
    let withheld = payloads(&pool, thread_id, "ProposalWithheld").await;
    assert_eq!(
        withheld.last().expect("announced")["reason"],
        "hook_refused"
    );
    assert_eq!(hook_refusals(&pool, "require-a-plan").await, 1);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_plan_shadow_hook_sees_both_proposal_verdicts_without_enforcing() {
    use crate::core::event_hooks::PROPOSAL_CHECK_EVENT;
    use crate::engine::git_ops::{record_planned, PlanMarkerKind};
    use crate::test_support::{hook_refusals, install_refusing_hook};

    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, thread_id) = thread_with_work(&bus).await;
    let repo_str = repo.to_string_lossy().to_string();
    let plan = files(&["docs/plans/example.md"]);
    let slug = "shadow-plan";
    let asker = install_refusing_hook(&bus, slug, PROPOSAL_CHECK_EVENT, "shadow refused").await;

    let hold = hold_of(&pool, &bus, &input(thread_id, &repo_str, "work", &plan)).await;
    assert_eq!(hold, Some(ProposalHold::Plan(PlanHold::Missing)));

    record_planned(
        &pool,
        &repo,
        BRANCH,
        PlanMarkerKind::Planned,
        None,
        None,
        &[],
        "abc",
    )
    .await
    .unwrap();
    let hold = hold_of(&pool, &bus, &input(thread_id, &repo_str, "work", &plan)).await;
    assert_eq!(hold, None);

    let asked = asker.asked();
    assert_eq!(asked.len(), 2);
    assert_eq!(
        asked[0].payload["change_context"]["plan_marker"]["state"],
        "missing"
    );
    assert_eq!(
        asked[1].payload["change_context"]["plan_marker"]["kind"],
        "planned"
    );
    assert_eq!(
        asked[1].payload["change_context"]["landing_files"],
        serde_json::json!(plan)
    );
    assert_eq!(hook_refusals(&pool, slug).await, 0);
    teardown_test_db(&db).await;
}
