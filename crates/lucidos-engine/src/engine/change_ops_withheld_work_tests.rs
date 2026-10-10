//! What a turn end does with work it does not propose (ADR 0400), at the
//! free functions the engine methods wrap: the plan hold, the unfinished turn,
//! and Bring back of an incomplete set-aside row.

use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::git_ops::git_cmd;
use crate::engine::thread_events::{CancelCause, EventChannel, EventMeta, ThreadEvent};
use crate::engine::thread_lifecycle::CodingAgentChangeState;
use crate::test_support::{
    make_repo_and_worktree, read_change_state, setup_test_db, start_cc_session, teardown_test_db,
};

const BRANCH: &str = "claude-code/withheld-work";

/// A coding-agent thread on `BRANCH`, with `a.txt` committed beyond main.
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

/// I3: a plan hold proposes nothing and announces exactly one
/// `ProposalWithheld`, carrying the branch, its files and the hold's reason.
/// Once the marker satisfies the floor, the proposal goes ahead unannounced.
#[tokio::test]
async fn a_plan_hold_announces_the_branch_files_and_reason_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, thread_id) = thread_with_work(&bus).await;
    let repo_str = repo.to_string_lossy().to_string();
    let work = files(&["a.txt"]);

    let hold = withhold_for_plan(&pool, &bus, &input(thread_id, &repo_str, "work", &work)).await;

    assert_eq!(hold, Some(PlanHold::Missing));
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
    assert_eq!(
        withhold_for_plan(&pool, &bus, &input(thread_id, &repo_str, "work", &work)).await,
        None
    );
    assert_eq!(
        payloads(&pool, thread_id, "ProposalWithheld").await.len(),
        1
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// I4: an unfinished turn that committed past a pending change withdraws it,
/// so Apply can never merge the partial commits. The work lands unproposed
/// for `turn_incomplete`, and the branch keeps every commit (I5).
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

/// I4: an unfinished turn that added nothing past a pending change leaves it
/// pending, since Apply would merge exactly what was proposed. Nothing is
/// announced, because the change still carries all the work.
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

/// I4: an amend or an auto-commit can move the branch and keep the files and
/// the description. A moved branch withdraws the change all the same.
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

/// I4: a set-aside change the stopped turn committed past is withdrawn too, so
/// Bring back cannot make the partial commits applicable.
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

/// The branch counts as moved unless the last idle that recorded a head
/// recorded its current one. No idle, or an unreadable head, counts as moved.
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

/// I4: an unfinished turn with no change on its branch creates none.
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

/// Bring back of an incomplete set-aside row withdraws it rather than making
/// it pending, surfaces the thread, and reads how the last turn ended. An
/// unfinished turn's work is then withheld, never proposed (decision 17).
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

/// The engine method routes an incomplete row through the withdrawal and then
/// re-decides with its answer. A plain `ChangeBroughtBack` there would make
/// unfinished work pending again.
#[test]
fn bring_back_redecides_an_incomplete_row_instead_of_restoring_it() {
    let src = include_str!("change_ops/set_aside.rs");
    let body = &src[src.find("pub(crate) async fn bring_back_change(").unwrap()..];
    let incomplete = &body[body.find("if change.incomplete {").unwrap()..];
    let arm = &incomplete[..incomplete.find("return Ok(());").unwrap()];
    let withdraw = arm
        .find("withdraw_for_redecision(")
        .expect("withdraws first");
    let redecide = arm
        .find("self.redecide_branch_work(")
        .expect("then re-decides");
    assert!(withdraw < redecide);
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

/// An unfinished turn routes through the withholding, never the proposal.
#[test]
fn propose_turn_work_withholds_every_unfinished_turn() {
    let src = include_str!("change_ops/propose.rs");
    let body = &src[src.find("pub(crate) async fn propose_turn_work(").unwrap()..];
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
