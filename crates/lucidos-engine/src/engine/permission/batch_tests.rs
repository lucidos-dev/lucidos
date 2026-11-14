use super::*;
use crate::engine::command_guard::SideEffectCategory;
use crate::engine::permission::{card_request, resolve_answer};
use crate::engine::thread_events::QuestionOption;
use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};
use crate::triggers::{TriggerConfig, TriggerRun};

fn device() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "owner-phone".into(),
    }
}

/// Ask a card on `thread` with these labels, and return its id and options.
async fn ask(bus: &EventBus, thread: Uuid, labels: &[&str]) -> (String, Vec<QuestionOption>) {
    ask_with(bus, thread, labels, false, None).await
}

async fn ask_with(
    bus: &EventBus,
    thread: Uuid,
    labels: &[&str],
    multi_select: bool,
    owner_approval: Option<OwnerApproval>,
) -> (String, Vec<QuestionOption>) {
    let options: Vec<_> = labels
        .iter()
        .map(|label| serde_json::json!({ "label": label }))
        .collect();
    let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(
        &serde_json::json!({ "questions": [{ "question": "Which way?", "options": options }] }),
    )
    .remove(0);
    let tool_use_id = format!("toolu-{}#q0", Uuid::new_v4());
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.clone(),
            cc_session_id: String::new(),
            options: parsed.options.clone(),
            question: parsed.question,
            worktree_path: None,
            multi_select,
            owner_approval,
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

fn answer_on(thread: Uuid, option: &str) -> BatchItemRequest {
    BatchItemRequest::CardAnswer {
        thread_id: thread,
        tool_use_id: None,
        option: option.into(),
    }
}

async fn event_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn refusal(result: Result<Batch, BatchError>) -> String {
    match result {
        Err(BatchError::Refused(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A batch carries exact picks from sub-threads at any depth: each card's id,
/// the picked option's id and label, and the card's digest. Its own digest
/// hashes the items (I5).
#[tokio::test]
async fn a_batch_holds_exact_picks_from_its_sub_threads() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let child = seed_coding_thread(&bus, Some(raiser)).await;
    let grandchild = seed_coding_thread(&bus, Some(child)).await;
    let (child_card, child_options) = ask(&bus, child, &["Left", "Right"]).await;
    let (grand_card, grand_options) = ask(&bus, grandchild, &["Yes", "No"]).await;

    let batch = build_batch(
        &pool,
        &Triggers::default(),
        raiser,
        &[answer_on(child, "right"), answer_on(grandchild, " Yes ")],
    )
    .await
    .unwrap();

    let [BatchItem::CardAnswer {
        thread_id: first_thread,
        tool_use_id: first_card,
        option_id: first_option,
        option_label: first_label,
        plan: first_plan,
        ..
    }, BatchItem::CardAnswer {
        tool_use_id: second_card,
        option_id: second_option,
        ..
    }] = batch.items.as_slice()
    else {
        panic!("two items: {:?}", batch.items);
    };
    assert_eq!(*first_thread, child);
    assert_eq!(*first_card, child_card);
    assert_eq!(*first_option, child_options[1].id);
    assert_eq!(first_label, "Right");
    assert_eq!(*first_plan, None, "no plan waits on that thread");
    assert_eq!(*second_card, grand_card);
    assert_eq!(*second_option, grand_options[0].id);
    assert_eq!(batch.digest, items_digest(&batch.items).unwrap());
    teardown_test_db(&db).await;
}

/// Each batch rule refuses the whole batch, names the item, and writes
/// nothing (I13, I15).
#[tokio::test]
async fn a_batch_refuses_what_breaks_a_rule_and_writes_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let child = seed_coding_thread(&bus, Some(raiser)).await;
    let sibling = seed_coding_thread(&bus, None).await;
    let multi = seed_coding_thread(&bus, Some(raiser)).await;
    let nested = seed_coding_thread(&bus, Some(raiser)).await;
    let idle = seed_coding_thread(&bus, Some(raiser)).await;
    ask(&bus, raiser, &["Go"]).await;
    ask(&bus, child, &["Left", "Right"]).await;
    ask(&bus, sibling, &["Go"]).await;
    ask_with(&bus, multi, &["A", "B"], true, None).await;
    let grandchild = seed_coding_thread(&bus, Some(nested)).await;
    ask(&bus, grandchild, &["Go"]).await;
    raise_batch(
        &bus,
        &Triggers::default(),
        nested,
        &[answer_on(grandchild, "Go")],
    )
    .await
    .unwrap();
    let before = event_count(&pool).await;

    let cases: Vec<(Vec<BatchItemRequest>, &str)> = vec![
        (vec![], "at least one item"),
        (
            vec![answer_on(child, "Left"); MAX_BATCH_ITEMS + 1],
            "at most",
        ),
        (vec![answer_on(raiser, "Go")], "its own card"),
        (vec![answer_on(sibling, "Go")], "not below this thread"),
        (vec![answer_on(multi, "A")], "exactly one option"),
        (vec![answer_on(nested, "Run all")], "no batch holds a batch"),
        (vec![answer_on(idle, "Go")], "no open card"),
        (vec![answer_on(child, "Up")], "offers no option \"Up\""),
        (
            vec![answer_on(child, "Left"), answer_on(child, "Right")],
            "already answers that card",
        ),
    ];
    for (requests, expected) in cases {
        let why = refusal(build_batch(&pool, &Triggers::default(), raiser, &requests).await);
        assert!(why.contains(expected), "{expected:?} not in {why:?}");
        assert!(
            matches!(
                raise_batch(&bus, &Triggers::default(), raiser, &requests).await,
                Err(BatchError::Refused(_))
            ),
            "{expected}"
        );
    }
    assert_eq!(event_count(&pool).await, before, "a refusal writes nothing");
    teardown_test_db(&db).await;
}

/// The engine words the card: one line per item and the two options, and
/// only the owner's Run all grants (I3, I15).
#[tokio::test]
async fn a_raised_batch_is_engine_worded_and_only_the_owner_runs_it() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let child = seed_coding_thread(&bus, Some(raiser)).await;
    ask(&bus, child, &["Left", "Right"]).await;

    let card = raise_batch(
        &bus,
        &Triggers::default(),
        raiser,
        &[answer_on(child, "Left")],
    )
    .await
    .unwrap();
    let request = card_request(&pool, raiser, &card).await.unwrap().unwrap();
    assert_eq!(request.action, Action::RunBatch);
    assert_eq!(request.batch.as_ref().map(|b| b.items.len()), Some(1));
    let question: String = sqlx::query_scalar(
        "SELECT payload->>'question' FROM events \
         WHERE event_type = 'UserQuestionAsked' AND payload->>'tool_use_id' = $1",
    )
    .bind(&card)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(question.starts_with("Run this answer once?"), "{question}");
    assert!(
        question.contains("answer \"Which way?\" with **Left**"),
        "{question}"
    );
    let run = request
        .choices
        .iter()
        .find(|(_, c)| matches!(c, PermissionChoice::Grant { .. }))
        .map(|(id, _)| id.clone())
        .unwrap();
    assert_eq!(
        request.choices.get(&run),
        Some(&PermissionChoice::Grant {
            lifetime: Lifetime::Once,
            holder: Holder::Thread { thread_id: raiser },
        }),
        "a batch grants once, to the thread that raised it"
    );

    let agent = crate::engine::tools::agent_tool_actor(raiser);
    let pick = AnswerKind::Selected { option_id: run };
    let by_agent = resolve_answer(
        &bus,
        raiser,
        &card,
        &request,
        &pick,
        Some(&agent),
        PickedBy::Tap,
    )
    .await
    .unwrap();
    assert_eq!(by_agent, Resolution::Ignored, "an agent never runs a batch");
    let by_owner = resolve_answer(
        &bus,
        raiser,
        &card,
        &request,
        &pick,
        Some(&device()),
        PickedBy::Tap,
    )
    .await
    .unwrap();
    assert!(matches!(by_owner, Resolution::Granted(_)));
    teardown_test_db(&db).await;
}

/// A plan repo: `branch` carries a committed plan, and `thread`'s session
/// works it with a proposed marker.
struct PlanRepo {
    _dir: tempfile::TempDir,
    repo: std::path::PathBuf,
    branch: String,
}

const PLAN_PATH: &str = "docs/plans/p.md";

impl PlanRepo {
    async fn new(pool: &sqlx::PgPool, thread: Uuid) -> Self {
        use crate::engine::git_ops::git_cmd;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        let branch = format!("claude-code/{thread}");
        for args in [
            vec!["init", "-q"],
            vec!["checkout", "-q", "-b", branch.as_str()],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "T"],
        ] {
            git_cmd(&args, &repo).await.unwrap();
        }
        let plan_repo = Self {
            _dir: dir,
            repo,
            branch,
        };
        plan_repo.commit_plan("v1").await;
        plan_repo.mark(pool).await;
        plan_repo
    }

    async fn commit_plan(&self, text: &str) {
        use crate::engine::git_ops::git_cmd;
        tokio::fs::create_dir_all(self.repo.join("docs/plans"))
            .await
            .unwrap();
        tokio::fs::write(self.repo.join(PLAN_PATH), text)
            .await
            .unwrap();
        git_cmd(&["add", "."], &self.repo).await.unwrap();
        let out = git_cmd(&["commit", "-q", "-m", text], &self.repo)
            .await
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    async fn mark(&self, pool: &sqlx::PgPool) {
        crate::engine::git_ops::record_planned(
            pool,
            &self.repo,
            &self.branch,
            crate::engine::git_ops::PlanMarkerKind::Proposed,
            Some(PLAN_PATH),
            None,
            &[],
            "0000000",
        )
        .await
        .unwrap();
    }
}

/// An approving pick on a thread with a proposed plan binds the committed
/// text. The approval gate accepts that pick while the text stands, refuses
/// it once the text changes, and a re-mark needs a new yes (I16).
#[tokio::test]
async fn a_batch_pick_binds_the_plan_text_it_showed() {
    use crate::engine::git_ops::{user_approved_plan, PlanApprovalError};
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let child = seed_coding_thread(&bus, Some(raiser)).await;
    let plan = PlanRepo::new(&pool, child).await;
    let (child_card, child_options) = ask(&bus, child, &["Approve", "Request changes"]).await;

    let changes = build_batch(
        &pool,
        &Triggers::default(),
        raiser,
        &[answer_on(child, "Request changes")],
    )
    .await
    .unwrap();
    let BatchItem::CardAnswer { plan: none, .. } = &changes.items[0] else {
        panic!("a card answer: {:?}", changes.items);
    };
    assert_eq!(*none, None, "a pick that does not approve binds no plan");

    let card = raise_batch(
        &bus,
        &Triggers::default(),
        raiser,
        &[answer_on(child, "Approve")],
    )
    .await
    .unwrap();
    let request = card_request(&pool, raiser, &card).await.unwrap().unwrap();
    let Some(BatchItem::CardAnswer { plan: bound, .. }) =
        request.batch.as_ref().map(|b| &b.items[0])
    else {
        panic!("a card answer: {:?}", request.batch);
    };
    let bound = bound.clone().expect("the approving pick binds the plan");
    assert_eq!(bound.plan_path, PLAN_PATH);
    assert_eq!(bound.branch_name, plan.branch);

    let run = request
        .choices
        .iter()
        .find(|(_, c)| matches!(c, PermissionChoice::Grant { .. }))
        .map(|(id, _)| id.clone())
        .unwrap();
    resolve_answer(
        &bus,
        raiser,
        &card,
        &request,
        &AnswerKind::Selected { option_id: run },
        Some(&device()),
        PickedBy::Tap,
    )
    .await
    .unwrap();
    assert_eq!(
        item_drift(
            &pool,
            &Triggers::default(),
            &request.batch.as_ref().unwrap().items[0]
        )
        .await,
        Ok(None)
    );
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: child_card,
            answer: AnswerKind::Selected {
                option_id: child_options[0].id.clone(),
            },
            quiet: false,
        },
        meta: EventMeta::with_actor(Some(device())),
    })
    .await
    .unwrap();
    let thread = child.to_string();
    user_approved_plan(&pool, &plan.repo, &plan.branch, Some(&thread))
        .await
        .expect("the bound text stands, so the batch's pick approves");

    plan.commit_plan("v2").await;
    assert!(matches!(
        user_approved_plan(&pool, &plan.repo, &plan.branch, Some(&thread)).await,
        Err(PlanApprovalError::PlanChanged)
    ));

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    plan.mark(&pool).await;
    assert!(matches!(
        user_approved_plan(&pool, &plan.repo, &plan.branch, Some(&thread)).await,
        Err(PlanApprovalError::NotApproved)
    ));
    teardown_test_db(&db).await;
}

async fn tap(bus: &EventBus, thread: Uuid, tool_use_id: &str, option_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: tool_use_id.into(),
            answer: AnswerKind::Selected {
                option_id: option_id.into(),
            },
            quiet: false,
        },
        meta: EventMeta::with_actor(Some(device())),
    })
    .await
    .unwrap();
}

/// Only a pick on the plan card itself approves. A tap on an engine card
/// never does. A batch that skipped its item binds nothing, so the owner's
/// later direct tap approves the plan as it stands.
#[tokio::test]
async fn only_a_pick_on_the_plan_card_itself_approves() {
    use crate::api::owner_approval::{ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL};
    use crate::engine::git_ops::{user_approved_plan, PlanApprovalError};
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let child = seed_coding_thread(&bus, Some(raiser)).await;
    let plan = PlanRepo::new(&pool, child).await;
    let thread = child.to_string();

    let target = seed_coding_thread(&bus, None).await;
    let labels = [ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL];
    let (engine_card, engine_options) =
        ask_with(&bus, child, &labels, false, Some(apply_approval(target))).await;
    tap(&bus, child, &engine_card, &engine_options[1].id).await;
    assert!(matches!(
        user_approved_plan(&pool, &plan.repo, &plan.branch, Some(&thread)).await,
        Err(PlanApprovalError::NotApproved)
    ));

    let (plan_card, plan_options) = ask(&bus, child, &["Approve", "Request changes"]).await;

    let no_triggers = Triggers::default();
    let card = raise_batch(&bus, &no_triggers, raiser, &[answer_on(child, "Approve")])
        .await
        .unwrap();
    let request = card_request(&pool, raiser, &card).await.unwrap().unwrap();
    let run = request
        .choices
        .iter()
        .find(|(_, c)| matches!(c, PermissionChoice::Grant { .. }))
        .map(|(id, _)| id.clone())
        .unwrap();
    let answer = AnswerKind::Selected { option_id: run };
    let resolution = resolve_answer(
        &bus,
        raiser,
        &card,
        &request,
        &answer,
        Some(&device()),
        PickedBy::Tap,
    )
    .await
    .unwrap();
    let Resolution::Granted(permission_id) = resolution else {
        panic!("the run option grants: {resolution:?}");
    };
    plan.commit_plan("v2").await;
    let item = &request.batch.as_ref().unwrap().items[0];
    let skipped = BatchItemOutcome::Skipped {
        reason: "the plan changed".into(),
    };
    record_item(&bus, raiser, permission_id, 0, item, skipped).await;

    tap(&bus, child, &plan_card, &plan_options[0].id).await;
    user_approved_plan(&pool, &plan.repo, &plan.branch, Some(&thread))
        .await
        .expect("the batch skipped its pick, so the direct tap approves");
    teardown_test_db(&db).await;
}

/// A drifted item reads as skipped with its reason: a closed card, changed
/// options, a changed plan, and a re-marked plan (I5).
#[tokio::test]
async fn an_item_whose_source_changed_is_named_as_drifted() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let closed = seed_coding_thread(&bus, Some(raiser)).await;
    let reworded = seed_coding_thread(&bus, Some(raiser)).await;
    let planned = seed_coding_thread(&bus, Some(raiser)).await;
    let (closed_card, closed_options) = ask(&bus, closed, &["Go"]).await;
    let (reworded_card, _) = ask(&bus, reworded, &["Go", "Stop"]).await;
    let plan = PlanRepo::new(&pool, planned).await;
    ask(&bus, planned, &["Approve"]).await;
    let no_triggers = Triggers::default();
    let batch = build_batch(
        &pool,
        &no_triggers,
        raiser,
        &[
            answer_on(closed, "Go"),
            answer_on(reworded, "Go"),
            answer_on(planned, "Approve"),
        ],
    )
    .await
    .unwrap();
    for item in &batch.items {
        assert_eq!(item_drift(&pool, &no_triggers, item).await, Ok(None));
    }

    bus.emit(BusEvent::Thread {
        thread_id: closed,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: closed_card,
            answer: AnswerKind::Selected {
                option_id: closed_options[0].id.clone(),
            },
            quiet: false,
        },
        meta: EventMeta::with_actor(Some(device())),
    })
    .await
    .unwrap();
    let parsed = crate::engine::agent_session::parse_ask_user_question_inputs(&serde_json::json!({
        "questions": [{ "question": "Which way now?", "options": [{ "label": "Go" }] }]
    }))
    .remove(0);
    bus.emit(BusEvent::Thread {
        thread_id: reworded,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: reworded_card,
            cc_session_id: String::new(),
            options: parsed.options,
            question: parsed.question,
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            unattested_act: None,
            permission_request: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    plan.commit_plan("v2").await;

    let reasons: Vec<String> = futures::future::join_all(
        batch
            .items
            .iter()
            .map(|item| item_drift(&pool, &no_triggers, item)),
    )
    .await
    .into_iter()
    .map(|r| r.unwrap().expect("drifted"))
    .collect();
    assert_eq!(
        reasons,
        [
            "the card is no longer open",
            "the card's question or options changed",
            "the plan's committed text changed",
        ]
    );
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    plan.mark(&pool).await;
    assert_eq!(
        item_drift(&pool, &Triggers::default(), &batch.items[2]).await,
        Ok(Some("the plan was marked again".into()))
    );
    teardown_test_db(&db).await;
}

/// Whether this process is the child that boots a whole engine. The parent
/// reruns `test` in a child with its own HOME, asserts it passed, and gets
/// `false`.
async fn in_engine_child(test: &str) -> bool {
    const CHILD: &str = "LUCIDOS_BATCH_ENGINE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let home = tempfile::tempdir().unwrap();
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("engine::permission::batch::tests::{test}"),
                "--nocapture",
            ])
            .env("RUST_MIN_STACK", "16777216")
            .env(CHILD, "1")
            .env("HOME", home.path())
            .env_remove("LUCIDOS_GATEWAY_DATA")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

/// A whole engine on `db`, with a mock model. Keep the workspace alive.
async fn boot_engine(db: &str) -> (Arc<LucidosEngine>, tempfile::TempDir) {
    use crate::llm::ProviderBuildContext;
    let workspace = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        LucidosEngine::new(
            workspace.path().to_path_buf(),
            &crate::test_support::test_db_url(db),
            ProviderBuildContext {
                default_model: "mock".into(),
                model_is_mock: true,
                vertex_project_id: String::new(),
                vertex_location: crate::llm::vertex::location_handle(String::new()),
                vertex_token_cache: None,
                model_registry: Default::default(),
                not_served: Default::default(),
                claude_gateway_access: None,
                chatgpt_plan_access: None,
            },
        )
        .await
        .unwrap(),
    );
    engine.set_self_arc(&engine);
    (engine, workspace)
}

/// Each `BatchItemRan` on `raiser`: its index and outcome, in order.
async fn receipts(pool: &sqlx::PgPool, raiser: Uuid) -> Vec<(i64, serde_json::Value)> {
    sqlx::query_as(
        "SELECT (payload->>'index')::bigint, payload->'outcome' FROM events \
         WHERE thread_id = $1 AND event_type = 'BatchItemRan' ORDER BY sequence",
    )
    .bind(raiser)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The receipts on `raiser`, once all `count` of them have landed.
async fn receipts_once(
    pool: &sqlx::PgPool,
    raiser: Uuid,
    count: usize,
) -> Vec<(i64, serde_json::Value)> {
    let mut ran = Vec::new();
    for _ in 0..200 {
        ran = receipts(pool, raiser).await;
        if ran.len() == count {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(ran.len(), count, "one receipt per item: {ran:?}");
    ran
}

/// The engine's answer path, end to end (I5). The owner's Run all spends the
/// grant once and answers each intact card as the owner. A card that closed
/// meanwhile is skipped, every item gets a receipt, and a second run does
/// nothing.
#[tokio::test]
async fn a_confirmed_batch_runs_once_and_skips_what_changed() {
    use crate::engine::agent_question::answer_pending_question;

    if !in_engine_child("a_confirmed_batch_runs_once_and_skips_what_changed").await {
        return;
    }
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_engine(&db).await;
    let bus = &engine.event_bus;
    let raiser = seed_coding_thread(bus, None).await;
    let open = seed_coding_thread(bus, Some(raiser)).await;
    let closing = seed_coding_thread(bus, Some(raiser)).await;
    let (open_card, open_options) = ask(bus, open, &["Left", "Right"]).await;
    let (closing_card, closing_options) = ask(bus, closing, &["Go"]).await;
    let card = raise_batch(
        bus,
        &engine.trigger_configs,
        raiser,
        &[answer_on(open, "Right"), answer_on(closing, "Go")],
    )
    .await
    .unwrap();
    answer_pending_question(
        &engine,
        closing,
        closing_card,
        AnswerKind::Selected {
            option_id: closing_options[0].id.clone(),
        },
        Some(device()),
    )
    .await;

    let request = card_request(&pool, raiser, &card).await.unwrap().unwrap();
    let run = request
        .choices
        .iter()
        .find(|(_, c)| matches!(c, PermissionChoice::Grant { .. }))
        .map(|(id, _)| id.clone())
        .unwrap();
    answer_pending_question(
        &engine,
        raiser,
        card.clone(),
        AnswerKind::Selected { option_id: run },
        Some(device()),
    )
    .await;

    let ran = receipts_once(&pool, raiser, 2).await;
    assert_eq!(ran[0], (0, serde_json::json!({ "kind": "applied" })));
    assert_eq!(ran[1].0, 1);
    assert_eq!(ran[1].1["kind"], "skipped");
    assert_eq!(ran[1].1["reason"], "the card is no longer open");

    let (answer, actor): (serde_json::Value, serde_json::Value) = sqlx::query_as(
        "SELECT payload->'answer', payload->'actor' FROM events \
         WHERE thread_id = $1 AND event_type = 'UserQuestionAnswered' \
           AND payload->>'tool_use_id' = $2",
    )
    .bind(open)
    .bind(&open_card)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(answer["option_id"], open_options[1].id.as_str());
    assert_eq!(
        actor["kind"], "device",
        "the item answers as the owner who confirmed"
    );

    let granted: Uuid = sqlx::query_scalar(
        "SELECT (payload->>'permission_id')::uuid FROM events \
         WHERE thread_id = $1 AND event_type = 'PermissionGranted'",
    )
    .bind(raiser)
    .fetch_one(&pool)
    .await
    .unwrap();
    run_batch(
        engine.clone(),
        raiser,
        request,
        Resolution::Granted(granted),
        Some(device()),
    )
    .await;
    assert_eq!(
        receipts(&pool, raiser).await.len(),
        2,
        "a spent batch never runs again"
    );
    teardown_test_db(&db).await;
}

/// A restart can cut a batch off: before its spend, or between items. Boot
/// names every item that never ran, spends a dangling confirm, and a second
/// boot adds nothing (I10).
#[tokio::test]
async fn boot_names_the_items_a_restart_cut_off() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let first = seed_coding_thread(&bus, Some(raiser)).await;
    let second = seed_coding_thread(&bus, Some(raiser)).await;
    ask(&bus, first, &["Go"]).await;
    ask(&bus, second, &["Go"]).await;
    let requests = [answer_on(first, "Go"), answer_on(second, "Go")];

    let confirm = |card: String| {
        let (bus, pool) = (&bus, &pool);
        async move {
            let request = card_request(pool, raiser, &card).await.unwrap().unwrap();
            let run = request
                .choices
                .iter()
                .find(|(_, c)| matches!(c, PermissionChoice::Grant { .. }))
                .map(|(id, _)| id.clone())
                .unwrap();
            let Resolution::Granted(id) = resolve_answer(
                bus,
                raiser,
                &card,
                &request,
                &AnswerKind::Selected { option_id: run },
                Some(&device()),
                PickedBy::Tap,
            )
            .await
            .unwrap() else {
                panic!("the owner's Run all grants");
            };
            (id, request)
        }
    };
    let (unspent, _) = confirm(
        raise_batch(&bus, &Triggers::default(), raiser, &requests)
            .await
            .unwrap(),
    )
    .await;
    let (halfway, halfway_request) = confirm(
        raise_batch(&bus, &Triggers::default(), raiser, &requests)
            .await
            .unwrap(),
    )
    .await;
    let permission = crate::engine::permission::permission_row(&pool, halfway)
        .await
        .unwrap()
        .unwrap();
    assert!(crate::engine::permission::spend(&bus, &permission).await);
    record_item(
        &bus,
        raiser,
        halfway,
        0,
        &halfway_request.batch.as_ref().unwrap().items[0],
        BatchItemOutcome::Applied,
    )
    .await;

    let receipts = |permission: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (i64, String)>(
                "SELECT (payload->>'index')::bigint, payload->'outcome'->>'kind' FROM events \
                 WHERE event_type = 'BatchItemRan' AND payload->>'permission_id' = $1 \
                 ORDER BY sequence",
            )
            .bind(permission.to_string())
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    close_cut_off_batches_on_startup(&bus).await;
    close_cut_off_batches_on_startup(&bus).await;

    assert_eq!(
        receipts(unspent).await,
        [(0, "skipped".to_string()), (1, "skipped".to_string())]
    );
    assert_eq!(
        receipts(halfway).await,
        [(0, "applied".to_string()), (1, "skipped".to_string())]
    );
    let ended: Option<String> =
        sqlx::query_scalar("SELECT end_reason FROM permissions WHERE permission_id = $1")
            .bind(unspent)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        ended.as_deref(),
        Some("spent"),
        "a dangling confirm is spent, unrun"
    );
    teardown_test_db(&db).await;
}

/// An intent trigger `id` that runs `intent` with the `slack` MCP server.
fn trigger(id: &str, intent: &str) -> TriggerConfig {
    TriggerConfig::from_created_payload(&serde_json::json!({
        "trigger_id": id,
        "name": format!("Trigger {id}"),
        "slug": format!("slug-{id}"),
        "schedule": ["0 0 8 * * *"],
        "timezone": "UTC",
        "run": { "type": "intent", "intent": intent },
        "mcp_servers": ["slack"],
    }))
    .unwrap()
}

fn registry(configs: impl IntoIterator<Item = TriggerConfig>) -> Triggers {
    std::sync::RwLock::new(configs.into_iter().map(|c| (c.id.clone(), c)).collect())
}

fn grant_on(trigger: &str, side_effects: &[&str], coding_agents: bool) -> BatchItemRequest {
    BatchItemRequest::TriggerGrant {
        trigger: trigger.into(),
        side_effects: side_effects.iter().map(|s| s.to_string()).collect(),
        coding_agents,
    }
}

/// A batch carries grants across several triggers, by id or slug. Each item
/// binds the trigger's definition digest and adds only what it lacks. The card
/// shows each trigger's intent, digest, grants and MCP servers (I17).
#[tokio::test]
async fn a_batch_carries_trigger_grants_bound_to_each_definition() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let first = trigger("t1", "Summarise the inbox every morning.");
    let mut second = trigger("t2", "Post the standup.");
    second.side_effect_grant = vec![SideEffectCategory::Email];
    let (first_digest, second_digest) = (first.definition_digest(), second.definition_digest());
    let triggers = registry([first, second]);

    let card = raise_batch(
        &bus,
        &triggers,
        raiser,
        &[
            grant_on("t1", &["email", "external_api"], true),
            grant_on("slug-t2", &["email", "cloud_cli"], false),
        ],
    )
    .await
    .unwrap();

    let request = card_request(&pool, raiser, &card).await.unwrap().unwrap();
    let items = request.batch.unwrap().items;
    assert_eq!(
        items,
        [
            BatchItem::TriggerGrant {
                trigger_id: "t1".into(),
                definition_digest: first_digest.clone(),
                side_effects: vec![SideEffectCategory::Email, SideEffectCategory::ExternalApi],
                coding_agents: true,
            },
            BatchItem::TriggerGrant {
                trigger_id: "t2".into(),
                definition_digest: second_digest,
                side_effects: vec![SideEffectCategory::CloudCli],
                coding_agents: false,
            },
        ],
        "each item adds only what its trigger lacks"
    );
    let question: String = sqlx::query_scalar(
        "SELECT payload->>'question' FROM events \
         WHERE event_type = 'UserQuestionAsked' AND payload->>'tool_use_id' = $1",
    )
    .bind(&card)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        question.starts_with("Run these 2 grants once?"),
        "{question}"
    );
    for shown in [
        "**Trigger t1**",
        &first_digest[..12],
        "Summarise the inbox every morning.",
        "Start coding agents",
        "Send email or messages",
        "MCP servers: slack",
        "**Trigger t2**",
        "Cloud CLI mutations",
    ] {
        assert!(question.contains(shown), "{shown} in {question}");
    }
    teardown_test_db(&db).await;
}

/// A trigger item naming a secret read, an unknown category, no new grant or
/// an unknown trigger refuses the batch. It writes nothing (I13, I17).
#[tokio::test]
async fn a_trigger_grant_refuses_a_secret_read_and_writes_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let mut held = trigger("t2", "Post the standup.");
    held.side_effect_grant = vec![SideEffectCategory::Email];
    let triggers = registry([trigger("t1", "Summarise."), held]);
    let before = event_count(&pool).await;

    let cases: [(Vec<BatchItemRequest>, &str); 7] = [
        (
            vec![grant_on("t1", &["secret_read"], false)],
            "never grantable",
        ),
        (
            vec![grant_on("t1", &["email", "Secret-Read"], true)],
            "never grantable",
        ),
        (
            vec![grant_on("t1", &["teleport"], false)],
            "not a side-effect category",
        ),
        (vec![grant_on("t1", &[], false)], "grants nothing"),
        (vec![grant_on("t2", &["email"], false)], "grants nothing"),
        (vec![grant_on("nope", &["email"], false)], "no trigger"),
        (
            vec![
                grant_on("t1", &["email"], false),
                grant_on("slug-t1", &["other"], false),
            ],
            "already grants to that trigger",
        ),
    ];
    for (requests, expected) in cases {
        let why = refusal(build_batch(&pool, &triggers, raiser, &requests).await);
        assert!(why.contains(expected), "{expected} in {why}");
        assert!(why.starts_with("Item "), "names the item: {why}");
        assert!(matches!(
            raise_batch(&bus, &triggers, raiser, &requests).await,
            Err(BatchError::Refused(_))
        ));
    }
    assert_eq!(event_count(&pool).await, before, "a refusal writes nothing");
    teardown_test_db(&db).await;
}

/// A trigger item runs only on the definition it showed. A changed or deleted
/// trigger skips the item. The grant lands as the confirming owner (I5, I17).
#[tokio::test]
async fn a_trigger_grant_runs_only_on_the_definition_it_showed() {
    use crate::engine::trigger_writes::TriggerRegistryWriter;

    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let triggers = registry([trigger("t1", "Summarise.")]);
    let batch = build_batch(
        &pool,
        &triggers,
        raiser,
        &[grant_on("t1", &["email"], true)],
    )
    .await
    .unwrap();
    let item = &batch.items[0];
    assert_eq!(item_drift(&pool, &triggers, item).await, Ok(None));

    let edit = |intent: &str| {
        triggers.write().unwrap().get_mut("t1").unwrap().run = TriggerRun::Intent {
            intent: intent.into(),
        };
    };
    edit("Delete the inbox.");
    assert_eq!(
        item_drift(&pool, &triggers, item).await,
        Ok(Some("the trigger's definition changed".into()))
    );
    let removed = triggers.write().unwrap().remove("t1").unwrap();
    assert_eq!(
        item_drift(&pool, &triggers, item).await,
        Ok(Some("the trigger no longer exists".into()))
    );
    triggers.write().unwrap().insert("t1".into(), removed);

    let workspace = tempfile::tempdir().unwrap();
    let write_lock = tokio::sync::Mutex::new(());
    let writer = TriggerRegistryWriter {
        event_bus: &bus,
        trigger_configs: &triggers,
        workspace_path: workspace.path(),
        write_lock: &write_lock,
    };
    edit("Delete the inbox.");
    assert_eq!(
        apply_trigger_grant(&writer, item, Some(device())).await,
        BatchItemOutcome::Skipped {
            reason: "the trigger changed while the batch ran".into()
        },
        "the write re-checks the digest under the registry lock"
    );
    edit("Summarise.");
    assert_eq!(
        apply_trigger_grant(&writer, item, Some(device())).await,
        BatchItemOutcome::Applied
    );
    let granted = triggers.read().unwrap().get("t1").cloned().unwrap();
    assert!(granted.coding_agent_grant);
    assert_eq!(granted.side_effect_grant, [SideEffectCategory::Email]);
    let by_owner: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events WHERE event_type = 'TriggerUpdated' \
                         AND payload::text LIKE '%owner-phone%')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(by_owner, "the grant lands as the owner who confirmed");
    teardown_test_db(&db).await;
}

/// A Lucidos Agent thread below `parent`. Only a chat thread asks a
/// command-guard card.
async fn seed_chat_child(bus: &EventBus, parent: Uuid) -> Uuid {
    let thread = Uuid::new_v4();
    let mut message = crate::test_support::message_received("work");
    if let ThreadEvent::MessageReceived {
        parent_thread_id, ..
    } = &mut message
    {
        *parent_thread_id = Some(parent);
    }
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: message,
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
    thread
}

/// Ask a command-guard card on `thread`, as the Lucidos Agent's guard does.
async fn ask_command(bus: &EventBus, thread: Uuid, request_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::CommandPermissionRequested {
            request_id: request_id.into(),
            tool_use_id: format!("toolu-{request_id}"),
            tool_name: "run_bash".into(),
            command: "git push origin main".into(),
            summary: "Push the branch.".into(),
            grant_head: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// A tool-permission card joins a batch as one exact pick among its own
/// buttons. An Always allow never does, and a card that closed drifts (I9,
/// I15).
#[tokio::test]
async fn a_batch_picks_a_tool_card_button_but_never_an_always_allow() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let child = seed_chat_child(&bus, raiser).await;
    let both = seed_chat_child(&bus, raiser).await;
    ask_command(&bus, child, "req-push").await;
    ask_command(&bus, both, "req-both").await;
    ask(&bus, both, &["Go"]).await;
    let no_triggers = Triggers::default();

    let batch = build_batch(
        &pool,
        &no_triggers,
        raiser,
        &[answer_on(child, "allow once")],
    )
    .await
    .unwrap();
    assert_eq!(
        batch.items,
        [BatchItem::PermissionCardAnswer {
            thread_id: child,
            lane: LegacyLane::Command,
            request_id: "req-push".into(),
            pick: LegacyPick::Once,
            option_label: "Allow once".into(),
        }]
    );
    let named = BatchItemRequest::CardAnswer {
        thread_id: both,
        tool_use_id: Some("req-both".into()),
        option: "Deny".into(),
    };
    let by_id = build_batch(&pool, &no_triggers, raiser, &[named])
        .await
        .unwrap();
    assert!(
        matches!(
            by_id.items.as_slice(),
            [BatchItem::PermissionCardAnswer {
                pick: LegacyPick::Deny,
                ..
            }]
        ),
        "{:?}",
        by_id.items
    );

    let card = legacy::open_cards(&pool, child).await.unwrap().remove(0);
    let always: Vec<String> = card
        .options()
        .into_iter()
        .filter(|(pick, _)| matches!(pick, LegacyPick::Narrow | LegacyPick::Broad))
        .map(|(_, option)| option.label)
        .collect();
    assert!(!always.is_empty(), "the card offers an Always allow");
    for label in always {
        let why =
            refusal(build_batch(&pool, &no_triggers, raiser, &[answer_on(child, &label)]).await);
        assert!(why.contains("never a batch item"), "{why}");
    }
    let why = refusal(build_batch(&pool, &no_triggers, raiser, &[answer_on(both, "Go")]).await);
    assert!(why.contains("2 open cards"), "{why}");
    let twice = [answer_on(child, "Allow once"), answer_on(child, "Deny")];
    let why = refusal(build_batch(&pool, &no_triggers, raiser, &twice).await);
    assert!(why.contains("already answers that card"), "{why}");

    let item = &batch.items[0];
    assert_eq!(item_drift(&pool, &no_triggers, item).await, Ok(None));
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::CommandPermissionResolved {
            request_id: "req-push".into(),
            allowed: false,
            reason: None,
            persist_scope: None,
        },
        meta: EventMeta::with_actor(Some(device())),
    })
    .await
    .unwrap();
    assert_eq!(
        item_drift(&pool, &no_triggers, item).await,
        Ok(Some("the card is no longer open".into()))
    );
    teardown_test_db(&db).await;
}

fn apply_approval(target: Uuid) -> OwnerApproval {
    OwnerApproval {
        verb: Action::Apply,
        target_thread_id: Some(target),
        target_workspace_id: None,
        snapshot_handle: None,
    }
}

/// An owner approval card joins a batch, worded from its typed approval and
/// never from the agent's question. An unverified caller's card stays out.
#[tokio::test]
async fn a_batch_answers_an_owner_approval_but_never_an_unverified_act() {
    use crate::api::owner_approval::{ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL};
    use crate::engine::thread_events::{ActorMode, UnattestedAct, UnattestedRequest};

    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let raiser = seed_coding_thread(&bus, None).await;
    let approving = seed_coding_thread(&bus, Some(raiser)).await;
    let unverified = seed_coding_thread(&bus, Some(raiser)).await;
    let target = seed_coding_thread(&bus, None).await;
    let labels = [ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL];
    ask_with(
        &bus,
        approving,
        &labels,
        false,
        Some(apply_approval(target)),
    )
    .await;
    let act = UnattestedAct {
        caller: MessageOrigin::UnattestedCaller {
            mode: ActorMode::Agent,
            user_agent: None,
            claimed_device_id: None,
            claimed_local_token: false,
            claimed_workspace: None,
        },
        request: UnattestedRequest::StartCodingAgent,
    };
    crate::engine::unattested_act::raise_card(
        &bus,
        unverified,
        "Let it start?".into(),
        act,
        EventChannel::ClaudeCode,
    )
    .await
    .unwrap();
    let no_triggers = Triggers::default();

    let why = refusal(
        build_batch(
            &pool,
            &no_triggers,
            raiser,
            &[answer_on(unverified, ALLOW_ONCE_LABEL)],
        )
        .await,
    );
    assert!(why.contains("unverified caller's act"), "{why}");

    let card = raise_batch(
        &bus,
        &no_triggers,
        raiser,
        &[answer_on(approving, ALLOW_ONCE_LABEL)],
    )
    .await
    .unwrap();
    let question: String = sqlx::query_scalar(
        "SELECT payload->>'question' FROM events \
         WHERE event_type = 'UserQuestionAsked' AND payload->>'tool_use_id' = $1",
    )
    .bind(&card)
    .fetch_one(&pool)
    .await
    .unwrap();
    let target_title = thread_title(&pool, target).await.unwrap();
    assert!(
        question.contains(&format!(
            "answer the owner approval that lets it apply a change from **{target_title}** once \
             with **{ALLOW_ONCE_LABEL}**."
        )),
        "{question}"
    );
    assert!(!question.contains("Which way?"), "{question}");
    teardown_test_db(&db).await;
}

/// A confirmed batch answers a tool card through its own resolver. It answers
/// an owner approval as the owner's device, so the approval is spendable (I9).
#[tokio::test]
async fn a_confirmed_batch_answers_tool_cards_and_owner_approvals_as_the_owner() {
    use crate::api::owner_approval::{try_live_owner_approval, ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL};
    use crate::engine::agent_question::answer_pending_question;
    use crate::engine::cc_permission::{DedupKey, PermissionAnswer, PermissionEntry};

    if !in_engine_child("a_confirmed_batch_answers_tool_cards_and_owner_approvals_as_the_owner")
        .await
    {
        return;
    }
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_engine(&db).await;
    let bus = &engine.event_bus;
    let raiser = seed_coding_thread(bus, None).await;
    let tool = seed_chat_child(bus, raiser).await;
    let approving = seed_coding_thread(bus, Some(raiser)).await;
    let target = seed_coding_thread(bus, None).await;
    ask_command(bus, tool, "req-push").await;
    let mut waiter = {
        let mut state = engine.pending_command_permission.lock().unwrap();
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        let key: DedupKey = (tool, "run_bash".into(), "{}".into());
        state.by_dedup_key.insert(
            key.clone(),
            PermissionEntry {
                grant_offer: Default::default(),
                thread_id: tool,
                request_id: "req-push".into(),
                tool_name: "run_bash".into(),
                input: serde_json::json!({}),
                tx,
            },
        );
        state.by_request_id.insert("req-push".into(), key);
        rx
    };
    let approval = apply_approval(target);
    let labels = [ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL];
    let (approval_card, _) = ask_with(bus, approving, &labels, false, Some(approval.clone())).await;
    let card = raise_batch(
        bus,
        &engine.trigger_configs,
        raiser,
        &[
            answer_on(tool, "Allow once"),
            answer_on(approving, ALLOW_ONCE_LABEL),
        ],
    )
    .await
    .unwrap();
    let request = card_request(&pool, raiser, &card).await.unwrap().unwrap();
    let run = request
        .choices
        .iter()
        .find(|(_, c)| matches!(c, PermissionChoice::Grant { .. }))
        .map(|(id, _)| id.clone())
        .unwrap();
    answer_pending_question(
        &engine,
        raiser,
        card,
        AnswerKind::Selected { option_id: run },
        Some(device()),
    )
    .await;

    let applied = serde_json::json!({ "kind": "applied" });
    assert_eq!(
        receipts_once(&pool, raiser, 2).await,
        [(0, applied.clone()), (1, applied)]
    );
    assert_eq!(
        waiter.recv().await.unwrap(),
        PermissionAnswer::from_decision(true)
    );
    let (allowed, scope, actor): (bool, Option<serde_json::Value>, serde_json::Value) =
        sqlx::query_as(
            "SELECT (payload->>'allowed')::bool, payload->'persist_scope', payload->'actor' \
             FROM events WHERE thread_id = $1 AND event_type = 'CommandPermissionResolved'",
        )
        .bind(tool)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(allowed);
    assert!(
        scope.is_none_or(|s| s.is_null()),
        "Allow once stores no grant line"
    );
    assert_eq!(actor["kind"], "device", "the card answers as the owner");
    assert_eq!(
        try_live_owner_approval(&pool, approving, &approval)
            .await
            .unwrap(),
        Some(approval_card),
        "the owner's batch answer makes the approval spendable"
    );
    teardown_test_db(&db).await;
}

fn classify_on(workspaces: &[&str], origin: &str, evidence: &str) -> BatchItemRequest {
    serde_json::from_value(serde_json::json!({
        "kind": "workspace_classification",
        "workspaces": workspaces,
        "origin": origin,
        "evidence": evidence,
    }))
    .unwrap()
}

/// A pool that never connects: classification items read nothing from it.
fn idle_pool() -> sqlx::PgPool {
    sqlx::PgPool::connect_lazy("postgres://127.0.0.1:1/never").unwrap()
}

/// I7: the card shows one line per group, with count, slugs, origin and the
/// agent's evidence, never one line per workspace.
#[tokio::test]
async fn a_classification_batch_shows_one_line_per_group() {
    let (pool, raiser, triggers) = (idle_pool(), Uuid::new_v4(), Triggers::default());
    let requests = [
        classify_on(
            &["tour-1", "tour-2", "tour-3", "tour-4"],
            "demo",
            "Made by the rig.",
        ),
        classify_on(&["notes"], "user", "The owner's **own** notes."),
    ];
    let batch = build_batch(&pool, &triggers, raiser, &requests)
        .await
        .unwrap();
    let question = word_batch(&pool, &triggers, &batch).await.unwrap();
    assert!(
        question.starts_with("Run these 2 items once?"),
        "{question}"
    );
    assert!(
        question.contains(
            "1. Classify **4 workspaces** (`tour-1`, `tour-2`, `tour-3` and 1 more) as **demo**"
        ),
        "{question}"
    );
    assert!(
        question.contains("\u{201c}Made by the rig\\.\u{201d}"),
        "{question}"
    );
    assert!(question.contains("2. Classify **1 workspace** (`notes`) as **user**"));
    assert!(
        !question.contains("**own**"),
        "evidence is inert: {question}"
    );
    assert_eq!(
        question.lines().filter(|l| l.contains("Classify")).count(),
        2
    );
}

#[tokio::test]
async fn a_classification_refuses_what_breaks_a_rule() {
    let (pool, raiser, triggers) = (idle_pool(), Uuid::new_v4(), Triggers::default());
    let many: Vec<String> = (0..=MAX_CLASSIFIED_WORKSPACES)
        .map(|i| format!("w-{i}"))
        .collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let cases = [
        (vec![classify_on(&["a"], "demo", "  ")], "evidence"),
        (vec![classify_on(&[], "demo", "why")], "1 to 200"),
        (vec![classify_on(&many, "demo", "why")], "1 to 200"),
        (
            vec![classify_on(&["Not A Slug"], "demo", "why")],
            "not a workspace slug",
        ),
        (vec![classify_on(&["a", "a"], "demo", "why")], "twice"),
        (
            vec![
                classify_on(&["a"], "demo", "why"),
                classify_on(&["b", "a"], "eval", "why"),
            ],
            "already classifies",
        ),
    ];
    for (requests, expected) in cases {
        let why = refusal(build_batch(&pool, &triggers, raiser, &requests).await);
        assert!(why.contains(expected), "{expected}: {why}");
    }
    let unknown = serde_json::from_value::<BatchItemRequest>(serde_json::json!({
        "kind": "workspace_classification", "workspaces": ["a"],
        "origin": "unknown", "evidence": "why",
    }));
    assert!(unknown.is_err(), "a classification never sets unknown");
}
