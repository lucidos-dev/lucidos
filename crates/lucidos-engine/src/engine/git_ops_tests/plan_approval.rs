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
        "Implement in a fresh session: Sonnet 5.5, high"
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

#[test]
fn approval_for_later_and_implementation_are_distinct() {
    use super::plan_approval::{approves_target, ApprovalTarget};
    let later = picked("Commit plan for later");
    assert!(!approves(&later));
    assert!(approves_target(&later, ApprovalTarget::PlanForLater));
    assert!(!approves_target(
        &picked("Approve here"),
        ApprovalTarget::PlanForLater
    ));
    assert!(approves_target(
        &typed("for later"),
        ApprovalTarget::PlanForLater
    ));
    assert!(!approves_target(
        &typed("for later?"),
        ApprovalTarget::PlanForLater
    ));
    assert!(!approves(&typed("for later")));
    assert!(!approves(&picked("Approve design")));
    assert!(!approves_target(&later, ApprovalTarget::Design));
    assert!(approves_target(
        &picked("Approve design"),
        ApprovalTarget::Design
    ));
    assert!(approves_target(
        &typed("approve design"),
        ApprovalTarget::Design
    ));
    assert!(!approves_target(
        &typed("approve design?"),
        ApprovalTarget::Design
    ));
}

#[test]
fn recommended_labels_preserve_the_approval_target() {
    use super::plan_approval::{approves_target, ApprovalTarget};
    assert!(approves_target(
        &picked("Commit plan for later (Recommended)"),
        ApprovalTarget::PlanForLater
    ));
    for label in [
        "Commit plan for later (Recommended)",
        "Commit plan for later  (Recommended)",
        "Commit plan for later(Recommended)",
        "Approve design(Recommended)",
        "Approve design  (Recommended)",
        "Request changes(Recommended)",
        "Request changes  (Recommended)",
        "Approve design (Recommended)",
        "Approve design and plan here (Recommended)",
        "Request changes (Recommended)",
    ] {
        assert!(!approves(&picked(label)), "{label}");
    }
    assert!(!approves_target(
        &picked("Request changes (Recommended)"),
        ApprovalTarget::Design
    ));
    assert!(!approves(&typed("approve (recommended)")));
}

#[test]
fn only_an_explicit_selected_variant_can_revise_a_proposal() {
    use super::plan_approval::picked_plan_variant;
    assert!(picked_plan_variant(&picked("Frontend only")));
    for label in [
        "Approve",
        "Approve (Recommended)",
        "Approve plan",
        "Yes, approve",
        "LGTM (Recommended)",
        "Go ahead (recommended)",
        "Approve here",
        "Continue here",
        "Approve design",
        "Approve design and plan here",
        "Commit plan for later",
        "Request changes",
        "Implement in a fresh session: a rung on codex",
        "",
    ] {
        assert!(!picked_plan_variant(&picked(label)), "{label}");
    }
    assert!(!picked_plan_variant(&typed("approve")));
    assert!(!picked_plan_variant(&HumanInput::Other));
}

/// A thread whose agent session works `BRANCH`, plus its bus.
struct Owned {
    bus: EventBus,
    thread: Uuid,
}

async fn session_for(pool: &sqlx::PgPool, branch: &str) -> Owned {
    session_of_kind(pool, branch, Default::default()).await
}

async fn session_of_kind(
    pool: &sqlx::PgPool,
    branch: &str,
    coding_agent_kind: crate::engine::agent_session::CodingAgentKind,
) -> Owned {
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-test".to_string(),
            branch: branch.to_string(),
            repo_id: None,
            coding_agent_kind,
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
        emitting_trigger_id: None,
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
                    unattested_act: None,
                    permission_request: None,
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
                    compose: None,
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

#[tokio::test]
async fn revised_variants_require_a_direct_plan_card_and_never_override_batch_binding() {
    use crate::engine::permission::batch::{Batch, BatchItem};
    use crate::engine::permission::{Action, Holder, Lifetime, PermissionRequest, PickedBy};
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    propose(&pool, &repo).await;
    owned
        .tap(
            &["Approve (Recommended)", "Frontend only"],
            "Frontend only",
            device(),
        )
        .await;
    assert!(
        user_picked_plan_variant(&pool, &owned.thread.to_string(), &repo, BRANCH)
            .await
            .unwrap()
    );
    owned
        .tap(&["Codex", "Claude Code"], "Codex", device())
        .await;
    assert!(
        !user_picked_plan_variant(&pool, &owned.thread.to_string(), &repo, BRANCH)
            .await
            .unwrap()
    );
    let (card, options) = owned.ask(&["Approve", "Frontend only"]).await;
    let selected = options[1].clone();
    owned
        .answer(
            &card,
            AnswerKind::Selected {
                option_id: selected.id.clone(),
            },
            device(),
        )
        .await;
    let marked_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT planned_at FROM planned_branches WHERE branch_name = $1")
            .bind(BRANCH)
            .fetch_one(&pool)
            .await
            .unwrap();
    let binding = PlanBinding {
        repo_root: canonical_repo_root(&repo),
        branch_name: BRANCH.into(),
        plan_path: "docs/plans/x.md".into(),
        marked_at,
        digest: "0".repeat(64),
    };
    let parent = session_for(&pool, "parent").await;
    let batch_card = "batch-plan-variant";
    parent
        .bus
        .emit(BusEvent::Thread {
            thread_id: parent.thread,
            event: ThreadEvent::UserQuestionAsked {
                tool_use_id: batch_card.into(),
                cc_session_id: String::new(),
                options: vec![],
                question: "Run the batch?".into(),
                worktree_path: None,
                multi_select: false,
                owner_approval: None,
                unattested_act: None,
                permission_request: Some(PermissionRequest {
                    action: Action::RunBatch,
                    breadth: None,
                    prepared: None,
                    choices: Default::default(),
                    lines: None,
                    unattested_act: None,
                    batch: Some(Batch {
                        items: vec![BatchItem::CardAnswer {
                            thread_id: owned.thread,
                            tool_use_id: card,
                            option_id: selected.id,
                            option_label: selected.label,
                            card_digest: "card".into(),
                            plan: Some(binding),
                        }],
                        digest: "batch".into(),
                    }),
                }),
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    parent
        .bus
        .emit(BusEvent::Thread {
            thread_id: parent.thread,
            event: ThreadEvent::PermissionGranted {
                permission_id: Uuid::new_v4(),
                action: Action::RunBatch,
                breadth: None,
                lifetime: Lifetime::Once,
                holder: Holder::Thread {
                    thread_id: parent.thread,
                },
                request_tool_use_id: batch_card.into(),
                picked_by: PickedBy::Tap,
                pattern: None,
            },
            meta: EventMeta::with_actor(device()),
        })
        .await
        .unwrap();
    assert!(
        !user_picked_plan_variant(&pool, &owned.thread.to_string(), &repo, BRANCH)
            .await
            .unwrap()
    );
    assert!(matches!(
        user_approved_plan_for(
            &pool,
            &repo,
            BRANCH,
            Some(&owned.thread.to_string()),
            ApprovalTarget::Implementation
        )
        .await,
        Err(PlanApprovalError::PlanChanged)
    ));
    pool.close().await;
    teardown_test_db(&db).await;
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

/// A git read that failed is unknown: the binding fails closed, but as an
/// unreadable plan, never as a changed one. That holds when git cannot
/// spawn, and when it runs and fails rather than finding no plan.
#[tokio::test]
async fn a_plan_git_cannot_read_is_unreadable_not_changed() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    propose(&pool, &repo).await;
    let marked_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT planned_at FROM planned_branches WHERE branch_name = $1")
            .bind(BRANCH)
            .fetch_one(&pool)
            .await
            .unwrap();
    let broken = repo.join("broken");
    std::fs::create_dir(&broken).unwrap();
    std::fs::write(broken.join(".git"), "gitdir: /nonexistent\n").unwrap();
    for root in [repo.join("gone"), broken] {
        let binding = PlanBinding {
            repo_root: root.to_string_lossy().into_owned(),
            branch_name: BRANCH.into(),
            plan_path: "docs/plans/x.md".into(),
            marked_at,
            digest: "0".repeat(64),
        };
        let held = super::plan_approval::binding_still_holds(&pool, &binding, &repo, BRANCH).await;
        assert!(
            matches!(held, Err(PlanApprovalError::PlanUnreadable(_))),
            "{}: {held:?}",
            root.display()
        );
    }
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

/// A repo holding `BRANCH`, as a thread's worktree branch would.
async fn repo_with_branch() -> (tempfile::TempDir, std::path::PathBuf) {
    let (tmp, repo) = make_test_repo().await;
    git_cmd(&["branch", BRANCH], &repo).await.unwrap();
    (tmp, repo)
}

#[tokio::test]
async fn a_lucidos_or_app_thread_off_its_tracked_branch_is_told_which_branch_applies() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = repo_with_branch().await;
    for kind in [
        crate::engine::agent_session::CodingAgentKind::Lucidos,
        crate::engine::agent_session::CodingAgentKind::App,
    ] {
        let thread = session_of_kind(&pool, BRANCH, kind)
            .await
            .thread
            .to_string();
        assert_eq!(
            tracked_branch_elsewhere(&pool, &repo, &thread, "phase6-side")
                .await
                .unwrap()
                .as_deref(),
            Some(BRANCH),
            "{kind:?}: Apply lands only the engine-named branch, so a side-branch marker strands"
        );
        assert_eq!(
            tracked_branch_elsewhere(&pool, &repo, &thread, BRANCH)
                .await
                .unwrap(),
            None
        );
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_tracked_branch_absent_from_the_repo_never_refuses() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let thread = session_for(&pool, BRANCH).await.thread.to_string();
    assert_eq!(
        tracked_branch_elsewhere(&pool, &repo, &thread, "renamed")
            .await
            .unwrap(),
        None,
        "a renamed tracked branch is taken over at the next spawn, and another repo never held it"
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_external_thread_or_a_threadless_caller_may_mark_any_branch() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = repo_with_branch().await;
    let external = session_of_kind(
        &pool,
        BRANCH,
        crate::engine::agent_session::CodingAgentKind::External,
    )
    .await;
    assert_eq!(
        tracked_branch_elsewhere(&pool, &repo, &external.thread.to_string(), "renamed")
            .await
            .unwrap(),
        None,
        "an external thread adopts a renamed branch at idle"
    );
    assert_eq!(
        tracked_branch_elsewhere(&pool, &repo, &Uuid::new_v4().to_string(), "any")
            .await
            .unwrap(),
        None,
        "a caller with no session has no tracked branch"
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_legacy_external_thread_reads_its_durable_flag() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = repo_with_branch().await;
    let legacy = session_for(&pool, BRANCH).await.thread;
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, coding_agent_is_external_repo) \
         VALUES ($1, TRUE) \
         ON CONFLICT (thread_id) DO UPDATE SET coding_agent_is_external_repo = TRUE",
    )
    .bind(legacy)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        tracked_branch_elsewhere(&pool, &repo, &legacy.to_string(), "renamed")
            .await
            .unwrap(),
        None,
        "a session older than coding_agent_kind still counts as external"
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_plan_only_pick_cannot_be_used_for_implementation() {
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    propose(&pool, &repo).await;
    owned
        .tap(
            &["Approve here", "Commit plan for later"],
            "Commit plan for later",
            device(),
        )
        .await;
    assert!(matches!(
        approve_plan(&pool, &repo, BRANCH, None).await,
        Err(PlanApprovalError::NotApproved)
    ));
    assert!(
        approve_plan_for(&pool, &repo, BRANCH, None, true)
            .await
            .unwrap()
            .unwrap()
            .plan_only
    );
    assert_eq!(
        plan_marker_state(&pool, &repo, BRANCH).await,
        PlanMarkerState::Present(PlanMarkerKind::PlanOnly)
    );
    propose(&pool, &repo).await;
    owned.tap(&CARD, "Approve", device()).await;
    assert!(matches!(
        approve_plan_for(&pool, &repo, BRANCH, None, true).await,
        Err(PlanApprovalError::NotApproved)
    ));
    assert!(approved(&pool, &repo).await);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn design_approval_never_grants_implementation_permission() {
    use crate::engine::thread_events::PlanKind;
    let (pool, db) = setup_test_db().await;
    let (_tmp, repo) = make_test_repo().await;
    let owned = session_for(&pool, BRANCH).await;
    record_planned_with_kind(
        &pool,
        &repo,
        BRANCH,
        PlanMarkerKind::Proposed,
        Some("docs/plans/example-design.md"),
        None,
        &[],
        "abc",
        PlanKind::Design,
    )
    .await
    .unwrap();
    owned
        .tap(
            &["Approve design", "Approve design and plan here"],
            "Approve design",
            device(),
        )
        .await;
    let result = approve_plan(&pool, &repo, BRANCH, None)
        .await
        .unwrap()
        .unwrap();
    assert!(result.plan_only);
    assert_eq!(
        plan_marker_state(&pool, &repo, BRANCH).await,
        PlanMarkerState::Present(PlanMarkerKind::PlanOnly)
    );
    propose(&pool, &repo).await;
    let stored: String =
        sqlx::query_scalar("SELECT kind FROM planned_branches WHERE branch_name = $1")
            .bind(BRANCH)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored, "implementation",
        "a new mark carries its explicit kind"
    );
    pool.close().await;
    teardown_test_db(&db).await;
}
