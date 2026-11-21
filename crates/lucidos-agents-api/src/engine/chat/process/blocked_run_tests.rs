//! A trigger run the guard blocks on a missing grant waits on the owner's
//! card in its own thread. Allow resumes that same turn, with its transcript
//! intact, as a run of the same trigger.

use crate::engine::agent_question::{answer_pending_question, AnswerResult};
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::permission::menu::{ALLOW_ONCE_LABEL, ALWAYS_ALLOW_LABEL};
use crate::engine::thread_events::{AnswerKind, MessageOrigin, TriggerInvocation};
use crate::engine::LucidosEngine;
use crate::llm::mock::MOCK_TOOL_CALL_SENTINEL;
use crate::scheduler::user_tasks::{in_trigger_run, run_awaits_owner};
use crate::test_support::{
    boot_mock_engine, run_on_engine_runtime, setup_test_db, teardown_test_db,
};
use crate::triggers::continue_key::FireTarget;
use crate::triggers::TriggerConfig;
use serde_json::json;
use uuid::Uuid;

/// A command the trigger channel's guard blocks, and the program grant that
/// would allow it.
const BLOCKED_COMMAND: &str = "curl -X POST https://example.com";
const PROGRAM_GRANT: &str = "Bash(curl:*)";

async fn create_trigger(engine: &LucidosEngine) -> TriggerConfig {
    let trigger_id = Uuid::new_v4().to_string();
    engine
        .trigger_registry_writer()
        .write_created_minting_slug(
            &trigger_id,
            json!({
                "trigger_id": trigger_id,
                "name": "Report trigger",
                "run": { "type": "intent", "intent": "Post the report" }
            }),
            "Report trigger",
            None,
        )
        .await
        .unwrap();
    engine.trigger_configs.read().unwrap()[&trigger_id].clone()
}

/// Fire `config` once, with the mock model calling the blocked command.
async fn fire(engine: &LucidosEngine, config: &TriggerConfig) -> Uuid {
    let args = json!({ "command": BLOCKED_COMMAND });
    let prompt = format!("{MOCK_TOOL_CALL_SENTINEL} run_bash {args}");
    let run = engine.process_trigger(
        &config.id,
        &config.name,
        &config.slug,
        &prompt,
        TriggerInvocation::Schedule,
        false,
        Vec::new(),
        None,
        None,
        None,
        Uuid::new_v4(),
        None,
        FireTarget::NewThread(None),
        None,
    );
    let error = in_trigger_run(config, Default::default(), run)
        .await
        .expect_err("the blocked run parks");
    assert!(run_awaits_owner(error.as_ref()), "{error}");
    sqlx::query_scalar("SELECT thread_id FROM thread_summaries WHERE trigger_id = $1")
        .bind(&config.id)
        .fetch_one(engine.pool())
        .await
        .unwrap()
}

async fn count(engine: &LucidosEngine, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = $2")
        .bind(thread_id)
        .bind(event_type)
        .fetch_one(engine.pool())
        .await
        .unwrap()
}

async fn status(engine: &LucidosEngine, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(engine.pool())
        .await
        .unwrap()
}

/// Pick the option labelled `label` on the run's card, as the owner.
async fn answer(engine: &std::sync::Arc<LucidosEngine>, thread_id: Uuid, label: &str) {
    let (card_id, option_id): (String, String) = sqlx::query_as(
        "SELECT payload->>'tool_use_id', o->>'id' FROM events, \
                jsonb_array_elements(payload->'options') o \
          WHERE thread_id = $1 AND event_type = 'UserQuestionAsked' AND o->>'label' = $2",
    )
    .bind(thread_id)
    .bind(label)
    .fetch_one(engine.pool())
    .await
    .unwrap();
    let owner = Some(MessageOrigin::Device {
        device_id: "test-device".into(),
    });
    let answer = AnswerKind::Selected { option_id };
    assert!(matches!(
        answer_pending_question(engine, thread_id, card_id, answer, owner).await,
        AnswerResult::Resumed,
    ));
}

/// The `request_event_id` of the run's reply, once it lands.
async fn reply_turn(engine: &LucidosEngine, thread_id: Uuid) -> Option<Uuid> {
    for _ in 0..100 {
        let turn: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT NULLIF(payload->>'request_event_id', '')::uuid FROM events \
             WHERE thread_id = $1 AND event_type = 'ResponseGenerated'",
        )
        .bind(thread_id)
        .fetch_optional(engine.pool())
        .await
        .unwrap();
        if let Some(turn) = turn {
            return turn;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the resumed run never replied");
}

/// How many `TriggerExecuted` the trigger recorded, once at least one lands.
/// These fires skip the executor, which records the parked fire, so the first
/// is the resumed run's.
async fn executions(engine: &LucidosEngine, config: &TriggerConfig) -> i64 {
    for _ in 0..100 {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE aggregate = 'trigger' AND aggregate_id = $1 \
             AND event_type = 'TriggerExecuted'",
        )
        .bind(&config.id)
        .fetch_one(engine.pool())
        .await
        .unwrap();
        if count > 0 {
            return count;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    0
}

async fn resumes_in_place(label: &'static str) {
    let (_pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let config = create_trigger(&engine).await;

    let thread_id = fire(&engine, &config).await;
    assert_eq!(status(&engine, thread_id).await, "waiting_for_user_answer");
    assert_eq!(count(&engine, thread_id, "ResponseFailed").await, 0);
    let started: Uuid = sqlx::query_scalar(
        "SELECT id FROM events WHERE thread_id = $1 AND event_type = 'TriggerStarted'",
    )
    .bind(thread_id)
    .fetch_one(engine.pool())
    .await
    .unwrap();

    answer(&engine, thread_id, label).await;

    assert_eq!(
        reply_turn(&engine, thread_id).await,
        Some(started),
        "the reply joins the parked turn"
    );
    assert_eq!(count(&engine, thread_id, "TriggerStarted").await, 1);
    assert_eq!(
        count(&engine, thread_id, "ToolCalled").await,
        1,
        "the transcript stays"
    );
    assert_eq!(count(&engine, thread_id, "ResponseFailed").await, 0);
    let runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_summaries WHERE trigger_id = $1")
            .bind(&config.id)
            .fetch_one(engine.pool())
            .await
            .unwrap();
    assert_eq!(runs, 1, "no new thread");
    assert_eq!(
        executions(&engine, &config).await,
        1,
        "the resumed run closes the fire"
    );
    let grants: Vec<String> = engine.trigger_configs.read().unwrap()[&config.id]
        .program_grants
        .iter()
        .map(|grant| grant.pattern.clone())
        .collect();
    let lasting: &[&str] = if label == ALWAYS_ALLOW_LABEL {
        &[PROGRAM_GRANT]
    } else {
        &[]
    };
    assert_eq!(grants, lasting);
    engine.pool().close().await;
    teardown_test_db(&db).await;
}

#[test]
fn allow_once_resumes_the_blocked_run_in_place() {
    run_on_engine_runtime(
        "engine::chat::process::blocked_run_tests::allow_once_resumes_the_blocked_run_in_place",
        || resumes_in_place(ALLOW_ONCE_LABEL),
    );
}

#[test]
fn always_allow_grants_the_trigger_and_resumes_in_place() {
    run_on_engine_runtime(
        "engine::chat::process::blocked_run_tests::always_allow_grants_the_trigger_and_resumes_in_place",
        || resumes_in_place(ALWAYS_ALLOW_LABEL),
    );
}
