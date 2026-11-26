use super::*;
use crate::engine::event_bus::BusEvent;
use crate::engine::permission::menu::DONT_ALLOW_LABEL;
use crate::engine::permission::{
    card_request, resolve_answer, revoke, Holder, Lifetime, PermissionChoice, PermissionRequest,
    PickedBy,
};
use crate::engine::thread_events::{AnswerKind, EventMeta, MessageOrigin, ThreadEvent};
use crate::test_support::{
    make_repo_and_worktree, seed_coding_thread, setup_test_db, teardown_test_db,
};
use std::path::{Path, PathBuf};

async fn gate(
    bus: &EventBus,
    machine: &Machine,
    thread_id: Uuid,
    worktree: &Path,
    asking: Asking<'_>,
) -> Result<ConfinementGate, BoxError> {
    let _gating = GATING.lock().await;
    decide(bus, machine, thread_id, worktree, asking, &_gating).await
}

fn phone() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "phone".into(),
    }
}

async fn gate_for(bus: &EventBus, thread: Uuid, card_id: Option<&str>) -> ConfinementGate {
    let asking = match card_id {
        Some(card_id) => Asking::Waiting { card_id },
        None => Asking::Owner,
    };
    gate(bus, &Machine::Available, thread, &no_worktree(), asking)
        .await
        .unwrap()
}

fn no_worktree() -> PathBuf {
    PathBuf::from("/nonexistent/worktree")
}

async fn asked(bus: &EventBus, thread: Uuid) -> String {
    match gate_for(bus, thread, None).await {
        ConfinementGate::Asked { card_id } => card_id,
        other => panic!("expected a card, got {other:?}"),
    }
}

async fn tap(bus: &EventBus, thread: Uuid, card_id: &str, label: &str) {
    let (request, answer) = record_answer(bus, thread, card_id, label).await;
    resolve_answer(
        bus,
        thread,
        card_id,
        &request,
        &answer,
        Some(&phone()),
        PickedBy::Tap,
    )
    .await
    .unwrap();
}

async fn record_answer(
    bus: &EventBus,
    thread: Uuid,
    card_id: &str,
    label: &str,
) -> (PermissionRequest, AnswerKind) {
    let request = card_request(bus.pool(), thread, card_id)
        .await
        .unwrap()
        .expect("the card carries its request");
    let wanted = confinement_card(thread)
        .options
        .into_iter()
        .find(|o| o.label == label)
        .expect("the card offers that label")
        .choice;
    let option_id = request
        .choices
        .iter()
        .find(|(_, choice)| **choice == wanted)
        .map(|(id, _)| id.clone())
        .unwrap();
    let answer = AnswerKind::Selected { option_id };
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: card_id.to_string(),
            answer: answer.clone(),
            quiet: false,
        },
        meta: EventMeta::with_actor(Some(phone())),
    })
    .await
    .unwrap();
    (request, answer)
}

fn permitted(gate: ConfinementGate) -> PermissionRow {
    match gate {
        ConfinementGate::Permitted(permission) => *permission,
        other => panic!("expected a permitted run, got {other:?}"),
    }
}

#[tokio::test]
async fn no_grant_asks_the_owner_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;

    let card = asked(&bus, thread).await;
    let request = card_request(&pool, thread, &card).await.unwrap().unwrap();
    assert_eq!(request.action, Action::RunConfinementTests);
    assert_eq!(
        asked(&bus, thread).await,
        card,
        "a second call joins the card"
    );
    assert!(matches!(
        gate_for(&bus, thread, Some(&card)).await,
        ConfinementGate::Asked { card_id } if card_id == card
    ));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn asking_nobody_raises_no_card() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let worktree = no_worktree();
    let quiet = || gate(&bus, &Machine::Available, thread, &worktree, Asking::Nobody);

    assert!(matches!(
        quiet().await.unwrap(),
        ConfinementGate::Unpermitted
    ));
    assert_eq!(
        tappable_request(&pool, thread, Action::RunConfinementTests)
            .await
            .unwrap(),
        None
    );
    let card = asked(&bus, thread).await;
    tap(&bus, thread, &card, ALWAYS_ALLOW_LABEL).await;
    permitted(quiet().await.unwrap());
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_card_the_agent_moved_past_is_still_joined() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::CodingAgentTextStreamed {
            text: "Polling the suites.".into(),
            coding_agent: crate::runtime::agent_runtime::CodingAgent::ClaudeCode,
            parent_tool_use_id: None,
            progress_note: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert!(matches!(
        gate_for(&bus, thread, Some(&card)).await,
        ConfinementGate::Asked { card_id } if card_id == card
    ));
    assert_eq!(asked(&bus, thread).await, card);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_once_grant_runs_once_then_asks_again() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    tap(&bus, thread, &card, ALLOW_ONCE_LABEL).await;

    let permission = permitted(gate_for(&bus, thread, Some(&card)).await);
    assert_eq!(permission.lifetime, "once");
    let next = asked(&bus, thread).await;
    assert_ne!(next, card, "the spent grant raises a fresh card");
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_grant_still_landing_keeps_the_caller_waiting() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    let (request, answer) = record_answer(&bus, thread, &card, ALLOW_ONCE_LABEL).await;

    assert!(matches!(
        gate_for(&bus, thread, Some(&card)).await,
        ConfinementGate::Asked { card_id } if card_id == card
    ));
    resolve_answer(
        &bus,
        thread,
        &card,
        &request,
        &answer,
        Some(&phone()),
        PickedBy::Tap,
    )
    .await
    .unwrap();
    assert_eq!(
        granting_answer(&bus, thread, &card).await.unwrap(),
        Some(GrantState::Landed),
        "a poll that read before the grant re-reads the permission"
    );
    permitted(gate_for(&bus, thread, Some(&card)).await);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_standing_grant_runs_every_time_until_revoked() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let other = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    tap(&bus, thread, &card, ALWAYS_ALLOW_LABEL).await;

    let permission = permitted(gate_for(&bus, thread, Some(&card)).await);
    permitted(gate_for(&bus, thread, None).await);
    permitted(gate_for(&bus, other, None).await);

    revoke(&bus, permission.permission_id, Some(phone()))
        .await
        .unwrap();
    asked(&bus, thread).await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_once_grant_covers_only_its_own_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let other = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    tap(&bus, thread, &card, ALLOW_ONCE_LABEL).await;

    asked(&bus, other).await;
    permitted(gate_for(&bus, thread, None).await);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn deny_closes_the_card_and_runs_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    tap(&bus, thread, &card, DONT_ALLOW_LABEL).await;
    assert_eq!(granting_answer(&bus, thread, &card).await.unwrap(), None);

    assert!(matches!(
        gate_for(&bus, thread, Some(&card)).await,
        ConfinementGate::Closed { card_id } if card_id == card
    ));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_agents_answer_grants_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    let request = card_request(&pool, thread, &card).await.unwrap().unwrap();
    for option_id in request.choices.keys() {
        let answer = AnswerKind::Selected {
            option_id: option_id.clone(),
        };
        resolve_answer(&bus, thread, &card, &request, &answer, None, PickedBy::Tap)
            .await
            .unwrap();
    }
    assert!(matches!(
        gate_for(&bus, thread, Some(&card)).await,
        ConfinementGate::Asked { .. }
    ));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_inherited_engine_refuses_and_asks_nobody() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;

    let verdict = gate(
        &bus,
        &Machine::Inherited,
        thread,
        &no_worktree(),
        Asking::Owner,
    )
    .await
    .unwrap();
    assert!(matches!(verdict, ConfinementGate::Inherited));
    assert_eq!(
        tappable_request(&pool, thread, Action::RunConfinementTests)
            .await
            .unwrap(),
        None
    );
    teardown_test_db(&db).await;
}

#[test]
fn the_card_names_the_risk() {
    let card = confinement_card(Uuid::new_v4());
    for words in ["no sandbox", "this branch's code", "Lucidos's secrets"] {
        assert!(card.question.contains(words), "{words}: {}", card.question);
    }
    let always = card
        .options
        .iter()
        .find(|o| o.label == ALWAYS_ALLOW_LABEL)
        .unwrap();
    for words in ["Every future confinement run", "until you revoke it"] {
        assert!(always.description.contains(words), "{words}");
    }
    assert!(matches!(
        always.choice,
        PermissionChoice::Grant {
            lifetime: Lifetime::Standing,
            holder: Holder::Workspace,
            ..
        }
    ));
}

async fn record_run(
    bus: &EventBus,
    thread: Uuid,
    task_id: &str,
    revision: Option<&str>,
    end: Option<TaskEndSeed>,
) {
    let emit = |event| async move {
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    };
    emit(ThreadEvent::ConfinementRunStarted {
        task_id: task_id.to_string(),
        permission_id: Uuid::new_v4(),
        revision: revision.map(str::to_string),
    })
    .await;
    if let Some(end) = end {
        emit(completed(task_id, end)).await;
    }
}

#[derive(Clone, Copy, Default)]
struct TaskEndSeed {
    exit_code: Option<i32>,
    timed_out: bool,
    killed: bool,
    abandoned: bool,
}

const PASS: TaskEndSeed = TaskEndSeed {
    exit_code: Some(0),
    timed_out: false,
    killed: false,
    abandoned: false,
};

fn completed(task_id: &str, end: TaskEndSeed) -> ThreadEvent {
    let now = chrono::Utc::now();
    ThreadEvent::BackgroundBashCompleted {
        task_id: task_id.to_string(),
        command: CONFINEMENT_COMMAND.to_string(),
        exit_code: end.exit_code,
        signal: None,
        stdout: String::new(),
        stderr: String::new(),
        started_at: now,
        finished_at: now,
        timed_out: end.timed_out,
        killed: end.killed,
        abandoned: end.abandoned,
    }
}

async fn nobody(bus: &EventBus, thread: Uuid, worktree: &std::path::Path) -> ConfinementGate {
    gate(bus, &Machine::Available, thread, worktree, Asking::Nobody)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_pass_on_the_current_commit_certifies_and_spends_no_grant() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, _repo, wt) = make_repo_and_worktree("evidence").await;
    let head = current_head_sha(&wt).await.unwrap();
    let thread = seed_coding_thread(&bus, None).await;
    let card = asked(&bus, thread).await;
    tap(&bus, thread, &card, ALLOW_ONCE_LABEL).await;
    record_run(&bus, thread, "t1", Some(&head), Some(PASS)).await;

    match nobody(&bus, thread, &wt).await {
        ConfinementGate::Passed(run) => assert_eq!(
            run,
            CertifiedRun {
                task_id: "t1".into(),
                revision: head
            }
        ),
        other => panic!("expected a pass, got {other:?}"),
    }
    let spent: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'PermissionSpent'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(spent, 0, "reading evidence spends no grant");
    assert_eq!(
        permitted(gate_for(&bus, thread, None).await).lifetime,
        "once",
        "the once grant is still live"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn evidence_that_does_not_fit_certifies_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, _repo, wt) = make_repo_and_worktree("evidence").await;
    let head = current_head_sha(&wt).await.unwrap();
    let cases: [(&str, Option<&str>, TaskEndSeed); 6] = [
        ("older commit", Some("0123abc"), PASS),
        ("dirty at start", None, PASS),
        (
            "failed",
            Some(&head),
            TaskEndSeed {
                exit_code: Some(1),
                ..PASS
            },
        ),
        (
            "killed",
            Some(&head),
            TaskEndSeed {
                killed: true,
                ..PASS
            },
        ),
        (
            "timed out",
            Some(&head),
            TaskEndSeed {
                timed_out: true,
                ..PASS
            },
        ),
        (
            "abandoned",
            Some(&head),
            TaskEndSeed {
                abandoned: true,
                ..PASS
            },
        ),
    ];
    for (name, revision, end) in cases {
        let thread = seed_coding_thread(&bus, None).await;
        record_run(&bus, thread, "t1", revision, Some(end)).await;
        assert!(
            matches!(
                nobody(&bus, thread, &wt).await,
                ConfinementGate::Unpermitted
            ),
            "{name}"
        );
    }
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_later_run_hides_an_earlier_pass() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, _repo, wt) = make_repo_and_worktree("evidence").await;
    let head = current_head_sha(&wt).await.unwrap();
    for later in [
        Some(TaskEndSeed {
            exit_code: Some(1),
            ..PASS
        }),
        None,
    ] {
        let thread = seed_coding_thread(&bus, None).await;
        record_run(&bus, thread, "first", Some(&head), Some(PASS)).await;
        assert!(matches!(
            nobody(&bus, thread, &wt).await,
            ConfinementGate::Passed(_)
        ));
        record_run(&bus, thread, "second", Some(&head), later).await;
        assert!(matches!(
            nobody(&bus, thread, &wt).await,
            ConfinementGate::Unpermitted
        ));
    }
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_new_commit_or_an_edit_voids_the_evidence() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, _repo, wt) = make_repo_and_worktree("evidence").await;
    let head = current_head_sha(&wt).await.unwrap();
    let thread = seed_coding_thread(&bus, None).await;
    record_run(&bus, thread, "t1", Some(&head), Some(PASS)).await;
    assert!(matches!(
        nobody(&bus, thread, &wt).await,
        ConfinementGate::Passed(_)
    ));

    std::fs::write(wt.join("seed.txt"), "edited").unwrap();
    assert!(matches!(
        nobody(&bus, thread, &wt).await,
        ConfinementGate::Unpermitted
    ));

    use crate::engine::git_ops::git_cmd;
    git_cmd(&["commit", "-am", "edit"], &wt).await.unwrap();
    assert!(matches!(
        nobody(&bus, thread, &wt).await,
        ConfinementGate::Unpermitted
    ));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_agent_started_task_is_not_evidence() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, _repo, wt) = make_repo_and_worktree("evidence").await;
    let thread = seed_coding_thread(&bus, None).await;
    for event in [
        ThreadEvent::BackgroundBashStarted {
            task_id: "t1".into(),
            command: CONFINEMENT_COMMAND.into(),
            timeout_secs: 60,
            started_at: chrono::Utc::now(),
        },
        completed("t1", PASS),
    ] {
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }
    assert!(matches!(
        nobody(&bus, thread, &wt).await,
        ConfinementGate::Unpermitted
    ));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_foreground_call_is_never_answered_from_evidence() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, _repo, wt) = make_repo_and_worktree("evidence").await;
    let head = current_head_sha(&wt).await.unwrap();
    let thread = seed_coding_thread(&bus, None).await;
    record_run(&bus, thread, "t1", Some(&head), Some(PASS)).await;

    let verdict = gate(&bus, &Machine::Available, thread, &wt, Asking::Owner)
        .await
        .unwrap();
    assert!(
        matches!(verdict, ConfinementGate::Asked { .. }),
        "{verdict:?}"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn git_config_cannot_hide_a_change_from_the_tree_state() {
    use crate::engine::git_ops::git_cmd;
    let (_tmp, _repo, wt) = make_repo_and_worktree("tree-state").await;
    let head = current_head_sha(&wt).await.unwrap();
    assert_eq!(tree_state(&wt).await, TreeState::Clean { head });

    git_cmd(&["config", "status.showUntrackedFiles", "no"], &wt)
        .await
        .unwrap();
    std::fs::write(wt.join("untracked.rs"), "fn hidden() {}").unwrap();
    assert_eq!(tree_state(&wt).await, TreeState::Dirty, "untracked file");
    std::fs::remove_file(wt.join("untracked.rs")).unwrap();

    for flag in ["--skip-worktree", "--assume-unchanged"] {
        git_cmd(&["update-index", flag, "seed.txt"], &wt)
            .await
            .unwrap();
        assert_eq!(tree_state(&wt).await, TreeState::Dirty, "{flag}");
        let undo = flag.replacen("--", "--no-", 1);
        git_cmd(&["update-index", &undo, "seed.txt"], &wt)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_tree_git_cannot_read_is_unknown() {
    let not_a_repo = tempfile::tempdir().unwrap();
    assert_eq!(tree_state(not_a_repo.path()).await, TreeState::Unknown);
}

#[test]
fn hides_paths_reads_the_skip_and_assume_tags() {
    assert!(!hides_paths(b"H a.rs\0H b.rs\0"));
    assert!(hides_paths(b"H a.rs\0S b.rs\0"));
    assert!(hides_paths(b"h a.rs\0"));
    assert!(!hides_paths(b""));
}

#[test]
fn only_a_clean_run_names_its_commit() {
    assert_eq!(
        confinement_command(&TreeState::Clean {
            head: "abc123".into()
        }),
        format!("{CONFINEMENT_COMMAND} --revision abc123")
    );
    for tree in [TreeState::Dirty, TreeState::Unknown] {
        assert_eq!(confinement_command(&tree), CONFINEMENT_COMMAND);
    }
}
