include!("../../../lucidos-engine/src/engine/automation_pause_test_actor.rs");

use super::*;
use crate::engine::permission::{Action, PermissionChoice};
use crate::engine::thread_events::{MessageOrigin, ThreadEvent};

/// The engine's answer path, end to end (I1, I3, I6). The step once grants
/// it once, spends it and resumes the thread with the step, still paused.
/// Keep paused does nothing. A follow-up never supersedes a request. Unpause
/// ends the pause and runs its step plus every other open request's step.
#[tokio::test]
async fn an_answered_step_request_runs_the_step_it_allows() {
    use crate::engine::agent_question::{answer_pending_question, AnswerResult};
    use crate::engine::permission::card_request;
    use crate::engine::thread_events::AnswerKind;
    use crate::{llm::ProviderBuildContext, test_support::*};
    use std::sync::Arc;

    const CHILD: &str = "LUCIDOS_AUTOMATION_PAUSE_ENGINE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir().unwrap();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "engine::automation_pause::tests::an_answered_step_request_runs_the_step_it_allows",
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
        return;
    }
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        crate::engine::LucidosEngine::new(
            workspace.path().to_path_buf(),
            &test_db_url(&db),
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
            std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
        )
        .await
        .unwrap(),
    );
    engine.set_self_arc(&engine);
    let bus = &engine.event_bus;
    let thread = seed_coding_thread(bus, None).await;
    seed_thread_event(
        bus,
        thread,
        ThreadEvent::ProposalWithheld {
            agent_returns: false,
            branch_name: format!("claude-code/{thread}"),
            files: vec!["a.txt".into()],
            reason: crate::engine::thread_events::UnproposedReason::PlanMissing,
        },
    )
    .await;
    pause_automation(&pool, bus, thread, "pause", None)
        .await
        .unwrap();

    let answer = |step: Action, pick: fn(&PermissionChoice) -> bool| {
        let engine = engine.clone();
        let pool = pool.clone();
        async move {
            ask_before_step(&engine.event_bus, thread, step).await;
            let card = open_step_request(&pool, thread, step)
                .await
                .unwrap()
                .expect("an open request");
            let request = card_request(&pool, thread, &card).await.unwrap().unwrap();
            let option_id = request
                .choices
                .iter()
                .find(|(_, choice)| pick(choice))
                .map(|(id, _)| id.clone())
                .unwrap();
            let result = answer_pending_question(
                &engine,
                thread,
                card,
                AnswerKind::Selected { option_id },
                device("phone"),
            )
            .await;
            assert!(matches!(result, AnswerResult::Resumed), "{step:?}");
        }
    };
    let resumes = || async {
        let mut reasons: Vec<String> = Vec::new();
        for _ in 0..50 {
            reasons = sqlx::query_scalar(
                "SELECT payload->>'reason' FROM events \
                 WHERE thread_id = $1 AND event_type = 'ContinuationRequested' ORDER BY sequence",
            )
            .bind(thread)
            .fetch_all(&pool)
            .await
            .unwrap();
            if !reasons.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        reasons
    };
    let count = |event_type: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = $2",
            )
            .bind(thread)
            .bind(event_type)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    answer(Action::Harden, |c| matches!(c, PermissionChoice::Refuse)).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        count("PermissionGranted").await,
        0,
        "Keep paused grants nothing"
    );
    assert_eq!(count("ContinuationRequested").await, 0);

    answer(Action::Harden, |c| {
        matches!(c, PermissionChoice::Grant { .. })
    })
    .await;
    assert_eq!(resumes().await, ["harden_requested"]);
    assert_eq!(count("PermissionGranted").await, 1);
    assert_eq!(count("PermissionSpent").await, 1, "the once grant is spent");
    assert!(automation_is_paused(&pool, thread).await, "still paused");

    ask_before_step(bus, thread, Action::Harden).await;
    let harden = open_step_request(&pool, thread, Action::Harden)
        .await
        .unwrap()
        .expect("a second harden request");
    let superseded = crate::engine::agent_question::resolve_pending_question_as_superseded(
        &engine,
        thread,
        device("phone"),
    )
    .await;
    assert!(!superseded, "a follow-up leaves a permission request open");
    assert_eq!(
        open_step_request(&pool, thread, Action::Harden)
            .await
            .unwrap(),
        Some(harden)
    );

    ask_before_step(bus, thread, Action::SetPlanMarker).await;
    assert_eq!(
        count("UserQuestionQueued").await,
        1,
        "it queues behind Harden"
    );
    assert!(open_step_request(&pool, thread, Action::SetPlanMarker)
        .await
        .unwrap()
        .is_some());
    answer(Action::Harden, |c| *c == PermissionChoice::Unpause).await;
    for _ in 0..50 {
        if count("ContinuationRequested").await == 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        resumes().await,
        ["harden_requested", "harden_requested", "plan_requested"],
        "Unpause also runs the step the queued request asked for"
    );
    assert_eq!(
        open_step_request(&pool, thread, Action::SetPlanMarker)
            .await
            .unwrap(),
        None,
        "and closes that request"
    );
    assert!(!automation_is_paused(&pool, thread).await);

    pool.close().await;
    teardown_test_db(&db).await;
}
