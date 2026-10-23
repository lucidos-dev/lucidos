//! `approve_plan` flips a proposed marker only on the user's own approval.

use super::common::make_test_repo;
use super::plan_approval::{approves, HumanInput};
use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, AnswerKind, EventChannel, EventMeta, MessageOrigin, QuestionOption, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

const BRANCH: &str = "feature";

fn typed(text: &str) -> HumanInput {
    HumanInput::Typed { text: text.into() }
}

fn picked(label: &str) -> HumanInput {
    HumanInput::Picked {
        label: Some(label.into()),
    }
}

#[test]
fn a_tapped_option_approves_unless_it_asks_for_changes() {
    assert!(approves(&picked("Approve")));
    assert!(approves(&picked("Approve here")));
    assert!(approves(&picked("Judge only")));
    assert!(approves(&picked(
        "Implement in a new thread: Sonnet 5.5, high"
    )));
    assert!(!approves(&picked("Request changes")));
    assert!(!approves(&picked(" request CHANGES ")));
    assert!(
        !approves(&HumanInput::Picked { label: None }),
        "an option that cannot be found is unknown, never approval"
    );
}

#[test]
fn a_typed_reply_approves_only_when_it_plainly_says_so() {
    for yes in [
        "approve",
        "Approved.",
        "  Approve it!  ",
        "approve the plan",
        "Yes",
        "yes approve",
        "LGTM",
        "go ahead",
    ] {
        assert!(approves(&typed(yes)), "{yes:?}");
    }
}

#[test]
fn a_question_or_a_counter_proposal_never_approves() {
    for no in [
        "can't we just update the agent and judge context for this? also it's just common sense?",
        "approve? or judge only",
        "approve?",
        "Yes?",
        "go ahead?",
        "yes but change X",
        "approve, then also fix the tests",
        "",
        "no",
    ] {
        assert!(!approves(&typed(no)), "{no:?}");
    }
    assert!(!approves(&HumanInput::Other));
}

/// A thread whose agent session works `BRANCH`, plus its bus.
struct Owned {
    bus: EventBus,
    thread: Uuid,
}

async fn session_for(pool: &sqlx::PgPool, branch: &str) -> Owned {
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".to_string(),
            branch: branch.to_string(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    Owned { bus, thread }
}

fn device() -> Option<MessageOrigin> {
    Some(MessageOrigin::Device {
        device_id: "owner-phone".into(),
    })
}

fn agent() -> Option<MessageOrigin> {
    Some(MessageOrigin::Api {
        user_agent: None,
        mode: ActorMode::Agent,
        source_thread_id: Some(Uuid::new_v4()),
    })
}

impl Owned {
    /// A plan card with these option labels, as the question walk stores it.
    async fn ask(&self, labels: &[&str]) -> (String, Vec<QuestionOption>) {
        let options: Vec<_> = labels
            .iter()
            .map(|label| serde_json::json!({ "label": label }))
            .collect();
        let input =
            serde_json::json!({ "questions": [{ "question": "Approve?", "options": options }] });
        let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(&input).remove(0);
        let tool_use_id = format!("toolu-{}#q0", Uuid::new_v4());
        self.bus
            .emit(BusEvent::Thread {
                thread_id: self.thread,
                event: ThreadEvent::UserQuestionAsked {
                    tool_use_id: tool_use_id.clone(),
                    cc_session_id: String::new(),
                    options: parsed.options.clone(),
                    question: parsed.question,
                    worktree_path: None,
                    multi_select: false,
                    owner_approval: None,
                },
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap();
        (tool_use_id, parsed.options)
    }

    async fn answer(&self, tool_use_id: &str, answer: AnswerKind, actor: Option<MessageOrigin>) {
        self.bus
            .emit(BusEvent::Thread {
                thread_id: self.thread,
                event: ThreadEvent::UserQuestionAnswered {
                    tool_use_id: tool_use_id.into(),
                    quiet: false,
                    answer,
                },
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    actor,
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap();
    }

    /// Ask a card and answer it with a tap on `label`.
    async fn tap(&self, labels: &[&str], label: &str, actor: Option<MessageOrigin>) {
        let (id, options) = self.ask(labels).await;
        let option_id = options
            .iter()
            .find(|o| o.label == label)
            .unwrap()
            .id
            .clone();
        self.answer(&id, AnswerKind::Selected { option_id }, actor)
            .await;
    }

    /// Ask a card and answer it with typed text.
    async fn type_reply(&self, text: &str, actor: Option<MessageOrigin>) {
        let (id, _) = self.ask(&["Approve", "Request changes"]).await;
        self.answer(
            &id,
            AnswerKind::FreeText {
                text: text.into(),
                image_hashes: vec![],
            },
            actor,
        )
        .await;
    }
}

async fn propose(pool: &sqlx::PgPool, repo: &std::path::Path) {
    record_planned(
        pool,
        repo,
        BRANCH,
        PlanMarkerKind::Proposed,
        Some("docs/plans/x.md"),
        None,
        &[],
        "abc",
    )
    .await
    .unwrap();
}

async fn approved(pool: &sqlx::PgPool, repo: &std::path::Path) -> bool {
    match approve_plan(pool, repo, BRANCH, None).await {
        Ok(flipped) => flipped.is_some(),
        Err(PlanApprovalError::NotApproved) => false,
        Err(e) => panic!("unexpected: {e}"),
    }
}

const CARD: [&str; 2] = ["Approve", "Request changes"];

#[tokio::test]
async fn a_typed_question_is_refused_and_leaves_the_marker_proposed() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    propose(&pool, &repo).await;
    owned
        .type_reply(
            "can't we just update the agent? it's just common sense?",
            device(),
        )
        .await;

    assert!(!approved(&pool, &repo).await);
    assert_eq!(
        plan_marker_state(&pool, &repo, BRANCH).await,
        PlanMarkerState::Present(PlanMarkerKind::Proposed),
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_approve_pick_and_a_plain_typed_approval_are_accepted() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    propose(&pool, &repo).await;
    owned.tap(&CARD, "Approve", device()).await;
    assert!(approved(&pool, &repo).await);

    propose(&pool, &repo).await;
    owned.type_reply("LGTM", device()).await;
    assert!(approved(&pool, &repo).await);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_fork_pick_is_accepted_and_request_changes_is_refused() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    propose(&pool, &repo).await;
    owned
        .tap(&["Approve", "Judge only"], "Judge only", device())
        .await;
    assert!(approved(&pool, &repo).await, "a fork is an approval");

    propose(&pool, &repo).await;
    owned.tap(&CARD, "Request changes", device()).await;
    assert!(!approved(&pool, &repo).await);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_answer_before_the_latest_mark_never_counts() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    propose(&pool, &repo).await;
    owned.tap(&CARD, "Approve", device()).await;

    // The agent re-marks to revise the plan: the old pick must not carry over.
    propose(&pool, &repo).await;
    assert!(!approved(&pool, &repo).await);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn only_a_devices_answer_counts() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    propose(&pool, &repo).await;
    owned.tap(&CARD, "Approve", agent()).await;
    assert!(!approved(&pool, &repo).await, "an agent actor");

    owned.tap(&CARD, "Approve", None).await;
    assert!(!approved(&pool, &repo).await, "no actor at all");
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_newest_input_decides() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    propose(&pool, &repo).await;
    owned.tap(&CARD, "Approve", device()).await;
    owned.tap(&CARD, "Request changes", device()).await;
    assert!(!approved(&pool, &repo).await);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn no_input_at_all_is_refused() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let _owned = session_for(&pool, BRANCH).await;
    propose(&pool, &repo).await;
    assert!(!approved(&pool, &repo).await);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_branch_no_session_started_still_approves() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let _other = session_for(&pool, "another-branch").await;
    propose(&pool, &repo).await;
    assert!(approved(&pool, &repo).await, "a human's own branch");
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn approving_a_branch_with_nothing_proposed_stays_a_no_op() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let _owned = session_for(&pool, BRANCH).await;
    assert!(
        !approved(&pool, &repo).await,
        "no row: nothing to approve, and no refusal either"
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

impl Owned {
    /// A message typed into the thread, as the composer records it.
    async fn say(&self, text: &str, origin: Option<MessageOrigin>) {
        self.bus
            .emit(BusEvent::Thread {
                thread_id: self.thread,
                event: ThreadEvent::MessageReceived {
                    provider: None,
                    voice_session_id: None,
                    wake_mode: None,
                    text: text.into(),
                    user_image_hashes: vec![],
                    device_id: None,
                    image_description: None,
                    parent_thread_id: None,
                    spawning_event_id: None,
                    mode: ActorMode::Human,
                    model: None,
                    reasoning_effort: None,
                    origin,
                },
                meta: EventMeta {
                    channel: Some(EventChannel::Chat),
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_typed_message_reads_like_a_typed_answer() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    propose(&pool, &repo).await;
    owned
        .say(
            "can't we just update the agent? it's just common sense?",
            device(),
        )
        .await;
    assert!(!approved(&pool, &repo).await, "a question is not approval");

    owned.say("approve", agent()).await;
    assert!(
        !approved(&pool, &repo).await,
        "an agent's message is not the user's"
    );

    owned.say("approve", device()).await;
    assert!(approved(&pool, &repo).await);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_callers_own_thread_is_read_even_after_a_branch_rename() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    let thread = owned.thread.to_string();
    record_planned(
        &pool,
        &repo,
        "renamed",
        PlanMarkerKind::Proposed,
        Some("docs/plans/x.md"),
        None,
        &[],
        "abc",
    )
    .await
    .unwrap();
    assert!(
        matches!(
            approve_plan(&pool, &repo, "renamed", Some(&thread)).await,
            Err(PlanApprovalError::NotApproved)
        ),
        "no input on the caller's thread, whatever the branch is called"
    );
    owned.tap(&CARD, "Approve", device()).await;
    assert!(approve_plan(&pool, &repo, "renamed", Some(&thread))
        .await
        .unwrap()
        .is_some());
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_superseded_card_never_hides_a_typed_approval() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    propose(&pool, &repo).await;
    let (stale, _) = owned.ask(&CARD).await;
    owned.say("approve", device()).await;
    owned.answer(&stale, AnswerKind::Superseded, device()).await;
    assert!(approved(&pool, &repo).await);
    pool.close().await;
    teardown_test_db(&db).await;
}
