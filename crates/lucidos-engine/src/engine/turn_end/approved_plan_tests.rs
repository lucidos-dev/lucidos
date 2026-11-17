use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::{AnswerKind, PlanKind, PlanScope, ThreadEvent};
use crate::test_support::{seed_coding_thread, seed_thread_event, setup_test_db, teardown_test_db};

fn approved(scope: PlanScope) -> ThreadEvent {
    ThreadEvent::PlanApproved {
        kind: PlanKind::Implementation,
        plan_path: "docs/plans/plan.md".into(),
        plan_sha: "abc".into(),
        scope,
    }
}

fn idled() -> ThreadEvent {
    serde_json::from_value(serde_json::json!({ "type": "CodingAgentIdled", "data": {} })).unwrap()
}

fn asked(id: &str) -> ThreadEvent {
    ThreadEvent::UserQuestionAsked {
        tool_use_id: id.into(),
        cc_session_id: String::new(),
        question: "Which way?".into(),
        options: vec![],
        worktree_path: None,
        multi_select: false,
        owner_approval: None,
        unattested_act: None,
        permission_request: None,
    }
}

fn answered(id: &str) -> ThreadEvent {
    ThreadEvent::UserQuestionAnswered {
        compose: None,
        tool_use_id: id.into(),
        quiet: false,
        answer: AnswerKind::Selected {
            option_id: "a".into(),
        },
    }
}

/// Only a full-scope approval since the last idle counts, and an open card
/// keeps the agent waiting for the user.
#[tokio::test]
async fn only_this_turns_full_approval_with_no_open_card_counts() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;

    assert!(!approved_this_turn_and_idle(&pool, thread).await);
    seed_thread_event(&bus, thread, approved(PlanScope::PlanOnly)).await;
    assert!(
        !approved_this_turn_and_idle(&pool, thread).await,
        "an approval for later builds nothing"
    );
    seed_thread_event(&bus, thread, approved(PlanScope::Implement)).await;
    assert!(approved_this_turn_and_idle(&pool, thread).await);

    seed_thread_event(&bus, thread, asked("q1")).await;
    assert!(
        !approved_this_turn_and_idle(&pool, thread).await,
        "an open card waits for the user"
    );
    seed_thread_event(&bus, thread, answered("q1")).await;
    assert!(approved_this_turn_and_idle(&pool, thread).await);

    seed_thread_event(&bus, thread, idled()).await;
    assert!(
        !approved_this_turn_and_idle(&pool, thread).await,
        "an earlier turn's approval does not count"
    );

    teardown_test_db(&db).await;
}

#[test]
fn only_a_known_diff_without_source_files_leaves_the_plan_unstarted() {
    let plan = vec!["docs/plans/plan.md".to_string()];
    let source = vec!["docs/plans/plan.md".to_string(), "src/main.rs".to_string()];
    assert!(no_source_change(Some(&[])));
    assert!(no_source_change(Some(&plan)));
    assert!(!no_source_change(Some(&source)));
    assert!(!no_source_change(None), "an unknown diff never sends back");
}

/// The session runner reads the approval before it writes this turn's idle.
/// After the idle, the query sees no approval and the re-entry never fires.
#[test]
fn the_runner_reads_the_approval_before_the_idle() {
    let run = include_str!("../agent_session/run_session/run.rs");
    let read = run
        .find("approved_plan::approved_this_turn_and_idle(")
        .expect("the runner reads the approval");
    let idle = run
        .find("self.emit_coding_agent_idled(")
        .expect("the runner writes the idle");
    assert!(
        read < idle,
        "the approval read must come before the idle write"
    );
}
